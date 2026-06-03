<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Window } from '@tauri-apps/api/window'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { useAiStore, type ModelOption } from '../stores/ai'
import { useRagStore } from '../stores/rag'
import MermaidBlock from './MermaidBlock.vue'
import { renderMarkdown, getSegments } from '../utils/renderMarkdown'
import { svgStringToPngBlob } from '../utils/svgToPng'
import { copyPngBlobToClipboard } from '../utils/clipboard'
import { buildChunks } from '../utils/chunker'
import type { ChatMessage, ModelSelection, RetrievedChunk, PaperIndexEntry, PaperVectorizeInput, ChunkInput } from '../types'

const emit = defineEmits<{ 'open-settings': [section?: 'ai' | 'rag'] }>()
const { t } = useI18n()
const ai = useAiStore()
const ragStore = useRagStore()

// ── RAG vectorization status ───────────────────────────────────────────────────
const allPapers = ref<PaperIndexEntry[]>([])
const syncingMissing = ref(false)
const syncProgress = ref({ done: 0, total: 0, failed: 0 })
const refreshingCounts = ref(false)
let syncCancelRequested = false

const vectorizedCount = computed(() => ragStore.storeInfo?.unique_papers ?? 0)
const unvectorizedPapers = computed(() => allPapers.value.filter(p => !p.status.vectorized))

async function loadPaperCounts() {
  try {
    allPapers.value = await invoke<PaperIndexEntry[]>('list_papers')
  } catch { /* no library open */ }
}

async function refreshCounts() {
  if (refreshingCounts.value) return
  refreshingCounts.value = true
  try {
    await invoke('sync_vectorized_flags')
    await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts()])
  } finally {
    refreshingCounts.value = false
  }
}

async function syncMissing() {
  if (syncingMissing.value || !ragStore.isConfigured) return
  syncingMissing.value = true
  syncCancelRequested = false
  const papers = unvectorizedPapers.value.slice()
  syncProgress.value = { done: 0, total: papers.length, failed: 0 }
  emitTo('main', 'rag-embed-progress', { syncing: true, done: 0, total: papers.length }).catch(() => {})

  const s = ragStore.settings
  let done = 0, failed = 0
  for (const paper of papers) {
    if (syncCancelRequested) break
    try {
      const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: paper.slug })
      const chunks: ChunkInput[] = buildChunks(input, s.chunk_size ?? 512, s.chunk_overlap ?? 50)
      if (chunks.length === 0) { failed++; syncProgress.value = { done, total: papers.length, failed }; continue }
      await invoke('embed_and_store_chunks', {
        slug: paper.slug, paperId: input.paper_id, paperTitle: input.paper_title, chunks,
      })
      paper.status.vectorized = true
      done++
    } catch { failed++ }
    syncProgress.value = { done, total: papers.length, failed }
    emitTo('main', 'rag-embed-progress', { syncing: true, done, total: papers.length }).catch(() => {})
  }
  syncingMissing.value = false
  emitTo('main', 'rag-embed-progress', { syncing: false, done, total: papers.length }).catch(() => {})
  await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts()])
}

// ── Types ─────────────────────────────────────────────────────────────────────

interface LibraryAnswerVariant {
  id: string
  content: string
  sources?: RetrievedChunk[]
  streaming?: boolean
  error?: boolean
  createdAt: string
  model?: ModelSelection | null
  modelLabel?: string
}

interface LibraryUiMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  sources?: RetrievedChunk[]
  streaming?: boolean
  error?: boolean
  createdAt: string
  model?: ModelSelection | null
  modelLabel?: string
  variants?: LibraryAnswerVariant[]
  activeVariantId?: string
}

interface LibraryConversation {
  id: string
  title: string
  messages: LibraryUiMessage[]
  createdAt: string
  updatedAt: string
}

interface GroupedSource {
  paper_id: string
  paper_title: string
  slug: string
  chunks: RetrievedChunk[]
}

// ── Storage ───────────────────────────────────────────────────────────────────

const STORAGE_KEY = 'argus.library-chats.v1'
const LAST_MODEL_KEY = 'argus.library-chat.last-model'

function genId() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 7)
}

function loadFromStorage(): LibraryConversation[] {
  try { return JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]') } catch { return [] }
}

function saveToStorage(convs: LibraryConversation[]) {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(convs.slice(0, 50))) } catch {}
}

// ── State ─────────────────────────────────────────────────────────────────────

const conversations = ref<LibraryConversation[]>([])
const activeConvId = ref<string | null>(null)
const input = ref('')
const loading = ref(false)
const messagesEl = ref<HTMLElement | null>(null)
const textareaEl = ref<HTMLTextAreaElement | null>(null)
const selectedModel = ref<ModelSelection | null>(null)
const modelMenuOpen = ref(false)
const modelMenuRoot = ref<HTMLElement | null>(null)
const expandedSources = ref<string[]>([])
const sidebarOpen = ref(true)
const editingMsgId = ref<string | null>(null)
const editingText = ref('')
const copiedMsgIds = ref(new Set<string>())

// ── Sidebar resize ─────────────────────────────────────────────────────────────
const SIDEBAR_WIDTH_KEY = 'argus:chat:sidebar-width'
const MIN_SIDEBAR_W = 160
const MAX_SIDEBAR_W = 420
const DEFAULT_SIDEBAR_W = 260

function loadSidebarWidth(): number {
  try {
    const v = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY))
    if (v >= MIN_SIDEBAR_W && v <= MAX_SIDEBAR_W) return v
  } catch {}
  return DEFAULT_SIDEBAR_W
}

const sidebarWidth = ref(loadSidebarWidth())
watch(sidebarWidth, (val) => {
  try { localStorage.setItem(SIDEBAR_WIDTH_KEY, String(Math.round(val))) } catch {}
})
let dividerDragging = false
let dividerStartX = 0
let dividerStartW = 0

function onDividerMouseDown(e: MouseEvent) {
  dividerDragging = true
  dividerStartX = e.clientX
  dividerStartW = sidebarWidth.value
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'col-resize'
  window.addEventListener('mousemove', onDividerMouseMove)
  window.addEventListener('mouseup', onDividerMouseUp)
}

function onDividerMouseMove(e: MouseEvent) {
  if (!dividerDragging) return
  const delta = e.clientX - dividerStartX
  sidebarWidth.value = Math.max(MIN_SIDEBAR_W, Math.min(MAX_SIDEBAR_W, dividerStartW + delta))
}

function onDividerMouseUp() {
  if (!dividerDragging) return
  dividerDragging = false
  document.body.style.userSelect = ''
  document.body.style.cursor = ''
  window.removeEventListener('mousemove', onDividerMouseMove)
  window.removeEventListener('mouseup', onDividerMouseUp)
  try { localStorage.setItem(SIDEBAR_WIDTH_KEY, String(Math.round(sidebarWidth.value))) } catch {}
}

let unlistenChat: UnlistenFn | null = null
let unlistenSources: UnlistenFn | null = null
let resizeTimer: ReturnType<typeof setTimeout> | null = null

const CHAT_WIN_SIZE_KEY = 'argus:chat:window:size'

function saveChatWindowSize() {
  if (resizeTimer) clearTimeout(resizeTimer)
  resizeTimer = setTimeout(() => {
    try {
      localStorage.setItem(CHAT_WIN_SIZE_KEY, JSON.stringify({
        width: window.innerWidth,
        height: window.innerHeight,
      }))
    } catch {}
  }, 400)
}

async function restoreChatWindowSize() {
  try {
    const raw = localStorage.getItem(CHAT_WIN_SIZE_KEY)
    if (!raw) return
    const { width, height } = JSON.parse(raw) as { width: number; height: number }
    if (!width || !height) return
    await getCurrentWebviewWindow().setSize(new LogicalSize(
      Math.max(560, width),
      Math.max(400, height),
    ))
  } catch {}
}
let pendingSources: RetrievedChunk[] = []
let _compositionEndedAt = 0

// ── Computed ──────────────────────────────────────────────────────────────────

const activeConv = computed(() =>
  conversations.value.find(c => c.id === activeConvId.value) ?? null
)
const activeMessages = computed(() => activeConv.value?.messages ?? [])
const canSend = computed(() => input.value.trim().length > 0 && !loading.value && ai.isConfigured)
const conversationSubtitle = computed(() => {
  if (!activeConv.value) return ''
  const count = userMsgCount(activeConv.value)
  return count > 0 ? `${formatDate(activeConv.value.updatedAt)} · ${count} 轮` : formatDate(activeConv.value.updatedAt)
})
const promptSuggestions = computed(() => [
  '帮我比较这些论文的共同方法',
  '哪些论文最适合作为 related work？',
  '总结这个文献库里的主要研究脉络',
  '找出和系统泛化最相关的论文',
])

function effectiveModel() { return selectedModel.value ?? ai.defaultSelection ?? null }

const selectedModelOption = computed(() => ai.findModel(effectiveModel()))

const modelSvgModules = import.meta.glob<{ default: string }>('/src/assets/models/*.svg', { eager: true })
const modelIconMap: Record<string, string> = {}
for (const [path, mod] of Object.entries(modelSvgModules)) {
  modelIconMap[path.replace(/^.*\//, '').replace(/\.svg$/, '')] = mod.default
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function deriveTitleFromMsg(text: string): string {
  const clean = text.replace(/\s+/g, ' ').trim()
  return clean.length > 42 ? clean.slice(0, 42) + '…' : clean
}

async function generateAiTitle(conv: LibraryConversation) {
  const userMsgs = conv.messages.filter((m: LibraryUiMessage) => m.role === 'user')
  const assistantMsgs = conv.messages.filter((m: LibraryUiMessage) => m.role === 'assistant')
  if (userMsgs.length !== 1 || assistantMsgs.length < 1) return
  const assistantMsg = assistantMsgs[0] as LibraryUiMessage
  const aiContent = activeAnswer(assistantMsg)?.content ?? assistantMsg.content
  if (!aiContent) return
  try {
    const title = await invoke<string>('generate_conversation_title', {
      userMsg: userMsgs[0].content,
      aiMsg: aiContent.slice(0, 400),
    })
    if (title?.trim()) {
      conv.title = title.trim().slice(0, 60)
      persistActive()
    }
  } catch { /* silently keep the derived title */ }
}

function formatDate(iso: string): string {
  const d = new Date(iso)
  const now = new Date()
  const days = Math.floor((now.getTime() - d.getTime()) / 86_400_000)
  if (days === 0) return t('libraryChat.today')
  if (days === 1) return t('libraryChat.yesterday')
  return d.toLocaleDateString()
}

function sourceTypeLabel(type: string): string {
  const map: Record<string, string> = {
    metadata: t('libraryChat.sourceMeta'),
    text: t('libraryChat.sourceText'),
    highlight: t('libraryChat.sourceHighlight'),
    note: t('libraryChat.sourceNote'),
  }
  return map[type] ?? type
}

function formatScore(score: number): string { return (score * 100).toFixed(0) + '%' }

function groupedSources(sources: RetrievedChunk[]): GroupedSource[] {
  const map = new Map<string, GroupedSource>()
  for (const chunk of sources) {
    if (!map.has(chunk.paper_id)) {
      map.set(chunk.paper_id, {
        paper_id: chunk.paper_id,
        paper_title: chunk.paper_title || chunk.slug,
        slug: chunk.slug,
        chunks: [],
      })
    }
    const group = map.get(chunk.paper_id)!
    const currentBest = group.chunks[0]
    if (!currentBest || chunk.score > currentBest.score) {
      group.chunks = [chunk]
    }
  }
  return [...map.values()]
}

function selectionKey(sel: ModelSelection | null | undefined) {
  return sel ? `${sel.providerId}::${sel.modelId}` : ''
}

function parseSelectionKey(key: string): ModelSelection | null {
  const sep = key.indexOf('::')
  if (sep === -1) return null
  return { providerId: key.slice(0, sep), modelId: key.slice(sep + 2) }
}

function validSelection(sel: ModelSelection | null) {
  return sel && ai.findModel(sel) ? sel : null
}

function restoreLastModel() {
  try {
    const saved = validSelection(parseSelectionKey(localStorage.getItem(LAST_MODEL_KEY) ?? ''))
    selectedModel.value = saved ?? ai.defaultSelection ?? ai.chatModels[0] ?? null
  } catch {
    selectedModel.value = ai.defaultSelection ?? ai.chatModels[0] ?? null
  }
}

function persistSelectedModel(sel: ModelSelection | null) {
  try {
    if (sel) localStorage.setItem(LAST_MODEL_KEY, selectionKey(sel))
    else localStorage.removeItem(LAST_MODEL_KEY)
  } catch {}
}

function modelLogo(model?: ModelOption | null) {
  if (!model) return ''
  const keys = [
    model.providerId,
    model.providerName,
    model.modelId.split('/')[0],
    model.displayName.split(':')[0],
  ]
  for (const raw of keys) {
    const key = raw.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
    if (modelIconMap[key]) return modelIconMap[key]
    if (key === 'anthropic') return modelIconMap.claude
    if (key === 'x-ai') return modelIconMap.xai || modelIconMap.grok
    if (key === 'google') return modelIconMap.gemini
  }
  return ''
}

function modelCapabilityText(model: ModelOption) {
  const map: Record<string, string> = {
    vision: '视觉',
    reasoning: '推理',
    tool_calling: '工具',
  }
  return model.capabilities
    .filter(cap => cap !== 'embedding')
    .map(cap => map[cap] ?? cap)
    .join(' · ')
}

function selectModel(model: ModelOption) {
  selectedModel.value = { providerId: model.providerId, modelId: model.modelId }
  modelMenuOpen.value = false
}

function selectedModelLabel() {
  return selectedModelOption.value?.displayName ?? '选择模型'
}

function modelLabel(sel: ModelSelection | null | undefined) {
  const model = ai.findModel(sel ?? null)
  return model?.displayName ?? sel?.modelId ?? '默认模型'
}

function activeAnswer(msg: LibraryUiMessage): LibraryAnswerVariant {
  const variants = msg.variants ?? []
  const active = variants.find(v => v.id === msg.activeVariantId) ?? variants[variants.length - 1]
  if (active) return active
  return {
    id: `${msg.id}:base`,
    content: msg.content,
    sources: msg.sources,
    streaming: msg.streaming,
    error: msg.error,
    createdAt: msg.createdAt,
    model: msg.model,
    modelLabel: msg.modelLabel,
  }
}

function answerSources(msg: LibraryUiMessage) {
  return activeAnswer(msg).sources ?? []
}

function answerVariants(msg: LibraryUiMessage): LibraryAnswerVariant[] {
  return msg.variants ?? []
}

function ensureAnswerVariants(msg: LibraryUiMessage) {
  if (!msg.variants || msg.variants.length === 0) {
    msg.variants = [{
      id: `${msg.id}:v0`,
      content: msg.content,
      sources: msg.sources,
      streaming: msg.streaming,
      error: msg.error,
      createdAt: msg.createdAt,
      model: msg.model,
      modelLabel: msg.modelLabel,
    }]
    msg.activeVariantId = msg.variants[0].id
  }
  return msg.variants
}

function chatHistoryFromMessages(messages: LibraryUiMessage[]): ChatMessage[] {
  return messages
    .map(m => {
      if (m.role === 'user') return { role: 'user', content: m.content }
      const ans = activeAnswer(m)
      if (ans.streaming || ans.error || !ans.content.trim()) return null
      return { role: 'assistant', content: ans.content }
    })
    .filter((m): m is ChatMessage => !!m)
}

function buildHistoryBeforeMessage(conv: LibraryConversation, messageId: string) {
  const idx = conv.messages.findIndex(m => m.id === messageId)
  return chatHistoryFromMessages(idx >= 0 ? conv.messages.slice(0, idx) : conv.messages)
}

function toggleSources(msgId: string) {
  const idx = expandedSources.value.indexOf(msgId)
  if (idx >= 0) expandedSources.value.splice(idx, 1)
  else expandedSources.value.push(msgId)
}

function isSourcesExpanded(msgId: string) { return expandedSources.value.includes(msgId) }

async function openSourcePaper(group: GroupedSource) {
  try {
    const mainWindow = await Window.getByLabel('main')
    await mainWindow?.show()
    await mainWindow?.setFocus()
  } catch {}
  await emitTo('main', 'argus-open-paper', {
    slug: group.slug,
    title: group.paper_title,
  }).catch(() => {})
}

function userMsgCount(conv: LibraryConversation) {
  return conv.messages.filter(m => m.role === 'user').length
}

function useSuggestion(text: string) {
  input.value = text
  nextTick(() => {
    textareaEl.value?.focus()
    autoResize()
  })
}

// ── Conversation management ───────────────────────────────────────────────────

function newConversation() {
  const conv: LibraryConversation = {
    id: genId(),
    title: t('libraryChat.untitled'),
    messages: [],
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  }
  conversations.value.unshift(conv)
  activeConvId.value = conv.id
  saveToStorage(conversations.value)
}

function selectConversation(id: string) { activeConvId.value = id }

function deleteConversation(id: string) {
  conversations.value = conversations.value.filter(c => c.id !== id)
  if (activeConvId.value === id) {
    if (conversations.value.length > 0) activeConvId.value = conversations.value[0].id
    else newConversation()
  }
  saveToStorage(conversations.value)
}

function persistActive() {
  if (!activeConv.value) return
  activeConv.value.updatedAt = new Date().toISOString()
  const idx = conversations.value.findIndex(c => c.id === activeConvId.value)
  if (idx > 0) {
    const [conv] = conversations.value.splice(idx, 1)
    conversations.value.unshift(conv)
  }
  saveToStorage(conversations.value)
}

// ── Messaging ─────────────────────────────────────────────────────────────────

function scrollToBottom() {
  nextTick(() => {
    if (messagesEl.value) messagesEl.value.scrollTop = messagesEl.value.scrollHeight
  })
}

function autoResize() {
  const el = textareaEl.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = Math.min(el.scrollHeight, 160) + 'px'
}

watch(input, () => nextTick(autoResize))

async function runAssistantRequest(
  conv: LibraryConversation,
  assistantMsg: LibraryUiMessage,
  target: LibraryUiMessage | LibraryAnswerVariant,
  history: ChatMessage[],
  sel: ModelSelection | null,
) {
  target.content = ''
  target.error = false
  target.streaming = true
  target.sources = undefined
  target.model = sel
  target.modelLabel = modelLabel(sel)
  assistantMsg.streaming = true
  loading.value = true
  pendingSources = []
  scrollToBottom()

  if (unlistenSources) { unlistenSources(); unlistenSources = null }
  unlistenSources = await listen<RetrievedChunk[]>('library-chat-sources', (e) => {
    pendingSources = e.payload ?? []
  })

  if (unlistenChat) { unlistenChat(); unlistenChat = null }
  unlistenChat = await listen<{ delta: string; done: boolean }>('library-chat', (e) => {
    if (!e.payload.done) { target.content += e.payload.delta; scrollToBottom() }
  })

  try {
    await invoke<string>('chat_with_library', {
      messages: history,
      providerId: sel?.providerId ?? null,
      modelId: sel?.modelId ?? null,
    })
    target.streaming = false
    assistantMsg.streaming = false
    if (pendingSources.length > 0) target.sources = [...pendingSources]
    persistActive()
    // Auto-generate title after the first exchange (fire-and-forget)
    if (conv.messages.filter((m: LibraryUiMessage) => m.role === 'user').length === 1) {
      generateAiTitle(conv)
    }
  } catch (e) {
    target.content = String(e)
    target.error = true
    target.streaming = false
    assistantMsg.streaming = false
  } finally {
    loading.value = false
    if (unlistenChat) { unlistenChat(); unlistenChat = null }
    if (unlistenSources) { unlistenSources(); unlistenSources = null }
    persistActive()
    scrollToBottom()
  }
}

function createAssistantMessage(sel: ModelSelection | null): LibraryUiMessage {
  return {
    id: genId(),
    role: 'assistant',
    content: '',
    streaming: true,
    createdAt: new Date().toISOString(),
    model: sel,
    modelLabel: modelLabel(sel),
  }
}

async function sendMessage() {
  const text = input.value.trim()
  if (!text || loading.value) return

  if (!activeConvId.value) newConversation()
  const conv = activeConv.value!
  const sel = effectiveModel()
  input.value = ''
  nextTick(autoResize)

  if (conv.messages.filter(m => m.role === 'user').length === 0) {
    conv.title = deriveTitleFromMsg(text)
  }

  conv.messages.push({
    id: genId(), role: 'user', content: text, createdAt: new Date().toISOString(),
  })
  const assistantMsg = createAssistantMessage(sel)
  conv.messages.push(assistantMsg)
  const history = buildHistoryBeforeMessage(conv, assistantMsg.id)
  await runAssistantRequest(conv, assistantMsg, assistantMsg, history, sel)
}

async function regenerateAssistant(msg: LibraryUiMessage, useCurrentModel = false) {
  if (loading.value || msg.role !== 'assistant' || !activeConv.value) return
  const variants = ensureAnswerVariants(msg)
  const current = activeAnswer(msg)
  const sel = useCurrentModel ? effectiveModel() : (current.model ?? msg.model ?? effectiveModel())
  const variant: LibraryAnswerVariant = {
    id: `${msg.id}:v${variants.length}`,
    content: '',
    createdAt: new Date().toISOString(),
    streaming: true,
    model: sel,
    modelLabel: modelLabel(sel),
  }
  variants.push(variant)
  msg.activeVariantId = variant.id
  const history = buildHistoryBeforeMessage(activeConv.value, msg.id)
  await runAssistantRequest(activeConv.value, msg, variant, history, sel)
}

function startEditUser(msg: LibraryUiMessage) {
  editingMsgId.value = msg.id
  editingText.value = msg.content
  nextTick(() => {
    const el = document.getElementById(`edit-${msg.id}`) as HTMLTextAreaElement | null
    el?.focus()
    el?.setSelectionRange(el.value.length, el.value.length)
  })
}

function cancelEdit() {
  editingMsgId.value = null
  editingText.value = ''
}

async function submitUserEdit(msg: LibraryUiMessage) {
  const text = editingText.value.trim()
  if (!text || loading.value || msg.role !== 'user' || !activeConv.value) return
  const conv = activeConv.value
  const idx = conv.messages.findIndex(m => m.id === msg.id)
  if (idx < 0) return
  msg.content = text
  conv.messages.splice(idx + 1)
  if (idx === 0) conv.title = deriveTitleFromMsg(text)
  cancelEdit()
  const sel = effectiveModel()
  const assistantMsg = createAssistantMessage(sel)
  conv.messages.push(assistantMsg)
  const history = buildHistoryBeforeMessage(conv, assistantMsg.id)
  await runAssistantRequest(conv, assistantMsg, assistantMsg, history, sel)
}

async function copyMessage(msg: LibraryUiMessage) {
  const text = msg.role === 'assistant' ? activeAnswer(msg).content : msg.content
  await navigator.clipboard.writeText(text).catch(() => {})
  copiedMsgIds.value.add(msg.id)
  copiedMsgIds.value = new Set(copiedMsgIds.value)
  setTimeout(() => {
    copiedMsgIds.value.delete(msg.id)
    copiedMsgIds.value = new Set(copiedMsgIds.value)
  }, 1400)
}

function onCompositionStart() { _compositionEndedAt = 0 }
function onCompositionEnd()   { _compositionEndedAt = Date.now() }
function isIMEActive()        { return Date.now() - _compositionEndedAt < 100 }

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey && !isIMEActive()) { e.preventDefault(); sendMessage() }
}

function onMsgContainerClick(e: MouseEvent) {
  const btn = (e.target as HTMLElement).closest('[data-action]') as HTMLElement | null
  if (!btn) return
  if (btn.dataset.action === 'copy-svg-image') {
    const svgEl = btn.closest('.svg-preview-area')?.querySelector('svg')
    if (svgEl) {
      const svgStr = new XMLSerializer().serializeToString(svgEl)
      svgStringToPngBlob(svgStr).then(copyPngBlobToClipboard).catch(() => {})
    }
  }
}

function closeModelMenu(e: MouseEvent) {
  if (modelMenuRoot.value && !modelMenuRoot.value.contains(e.target as Node)) {
    modelMenuOpen.value = false
  }
}

watch(selectedModel, (sel) => {
  if (ai.loaded) persistSelectedModel(sel)
})

watch(() => ai.chatModels.map(selectionKey).join('|'), () => {
  if (!ai.loaded) return
  if (!validSelection(selectedModel.value)) restoreLastModel()
})

onMounted(async () => {
  const saved = loadFromStorage()
  conversations.value = saved
  if (saved.length > 0) activeConvId.value = saved[0].id
  else newConversation()

  if (!ai.loaded) await ai.load()
  restoreLastModel()
  if (!ragStore.loaded) await ragStore.load()
  await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts()])
  document.addEventListener('mousedown', closeModelMenu)

  messagesEl.value?.addEventListener('copy-code', (e: Event) => {
    navigator.clipboard.writeText((e.target as HTMLElement).textContent ?? '').catch(() => {})
  })

  // Persist window size on resize (localStorage, same pattern as MainView)
  window.addEventListener('resize', saveChatWindowSize)
  restoreChatWindowSize()
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeModelMenu)
  window.removeEventListener('resize', saveChatWindowSize)
  window.removeEventListener('mousemove', onDividerMouseMove)
  window.removeEventListener('mouseup', onDividerMouseUp)
  if (unlistenChat) unlistenChat()
  if (unlistenSources) unlistenSources()
  if (resizeTimer) clearTimeout(resizeTimer)
})
</script>

<template>
  <div class="lc-root">

    <!-- ── Unified titlebar (full-width, drag region) ───────────────────────── -->
    <div class="lc-titlebar" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <template v-if="ai.loaded && ai.isConfigured">
        <div class="header-avatar" data-tauri-drag-region>
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round" data-tauri-drag-region>
            <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
            <path d="M8 9h8"/><path d="M8 13h5"/>
          </svg>
        </div>
        <div class="header-title-block" data-tauri-drag-region>
          <span class="header-conv-title" data-tauri-drag-region>{{ activeConv?.title || t('libraryChat.untitled') }}</span>
          <span class="header-subtitle" data-tauri-drag-region>{{ conversationSubtitle }}</span>
        </div>
        <div class="lc-titlebar-fill" data-tauri-drag-region />
        <div class="lc-titlebar-actions">
          <!-- RAG not configured -->
          <button v-if="!ragStore.isConfigured" class="rag-badge inactive" title="点击配置 RAG" @click="emit('open-settings', 'rag')">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>
            RAG
          </button>
          <!-- RAG configured -->
          <template v-else>
            <span v-if="syncingMissing" class="rag-sync-progress">{{ syncProgress.done }}/{{ syncProgress.total }}</span>
            <button class="rag-refresh-btn" :class="{ refreshing: refreshingCounts || syncingMissing }" title="刷新嵌入状态" :disabled="refreshingCounts || syncingMissing" @click="refreshCounts">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><polyline points="21 3 21 9 15 9"/></svg>
            </button>
            <div class="rag-counter" title="向量库：已嵌入论文 / 总论文数">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>
              <span class="rag-counter-text">{{ vectorizedCount }}/{{ allPapers.length }}</span>
            </div>
            <template v-if="syncingMissing">
              <button class="rag-sync-cancel" @click="syncCancelRequested = true" title="取消同步"><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg></button>
            </template>
            <button v-else class="rag-sync-btn" :class="{ 'all-done': unvectorizedPapers.length === 0 }" :title="unvectorizedPapers.length > 0 ? `嵌入 ${unvectorizedPapers.length} 篇未向量化的论文` : '所有论文已嵌入'" :disabled="unvectorizedPapers.length === 0" @click="syncMissing">
              <svg v-if="unvectorizedPapers.length > 0" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="16 16 12 12 8 16"/><line x1="12" y1="12" x2="12" y2="21"/><path d="M20.39 18.39A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.3"/></svg>
              <svg v-else width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg>
              {{ unvectorizedPapers.length > 0 ? `嵌入 ${unvectorizedPapers.length} 篇` : '已全部嵌入' }}
            </button>
          </template>
          <div ref="modelMenuRoot" class="lc-model-picker">
            <button class="lc-model-trigger" @click.stop="modelMenuOpen = !modelMenuOpen">
              <span class="lc-model-icon">
                <img v-if="modelLogo(selectedModelOption)" :src="modelLogo(selectedModelOption)" alt="" />
                <span v-else>{{ selectedModelLabel().charAt(0).toUpperCase() }}</span>
              </span>
              <span class="lc-model-label">{{ selectedModelLabel() }}</span>
              <svg class="chevron" :class="{ open: modelMenuOpen }" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="m6 9 6 6 6-6"/></svg>
            </button>
            <div v-if="modelMenuOpen" class="lc-model-menu">
              <div v-for="group in ai.groupedModels" :key="group.id" class="lc-model-group">
                <div class="lc-model-group-name">{{ group.name }}</div>
                <button v-for="model in group.models" :key="selectionKey(model)" class="lc-model-row" :class="{ active: selectionKey(model) === selectionKey(effectiveModel()) }" @click="selectModel(model)">
                  <span class="lc-model-row-icon"><img v-if="modelLogo(model)" :src="modelLogo(model)" alt="" /><span v-else>{{ model.displayName.charAt(0).toUpperCase() }}</span></span>
                  <span class="lc-model-row-text"><span class="lc-model-row-name">{{ model.displayName }}</span><span class="lc-model-row-meta">{{ modelCapabilityText(model) || model.modelId }}</span></span>
                </button>
              </div>
            </div>
          </div>
        </div>
      </template>
      <template v-else>
        <div class="lc-titlebar-fill" data-tauri-drag-region />
      </template>
    </div>

    <!-- ── No AI provider ────────────────────────────────────────────────────── -->
    <div v-if="ai.loaded && !ai.isConfigured" class="center-hint">
      <div class="hint-icon">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <circle cx="12" cy="12" r="9"/><path d="M12 8v4"/><path d="M12 16h.01"/>
        </svg>
      </div>
      <p class="hint-title">{{ t('copilot.noProviderTitle') }}</p>
      <p class="hint-desc">{{ t('copilot.noProviderDesc') }}</p>
      <button class="btn-primary" @click="emit('open-settings', 'ai')">{{ t('copilot.goToSettings') }}</button>
    </div>

    <template v-else>
      <!-- ── Body: sidebar + main ───────────────────────────────────────────── -->
      <div class="lc-body">
      <!-- ── Sidebar ─────────────────────────────────────────────────────────── -->
      <aside class="lc-sidebar" :style="{ width: sidebarWidth + 'px', minWidth: sidebarWidth + 'px' }">
        <div class="sidebar-header">
          <div class="sidebar-heading">
            <span class="sidebar-title">{{ t('libraryChat.historyTitle') }}</span>
            <span class="sidebar-count">{{ conversations.length }}</span>
          </div>
          <button class="new-chat-btn" :title="t('libraryChat.newChat')" @click="newConversation">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
            <span>{{ t('libraryChat.newChat') }}</span>
          </button>
        </div>

        <div class="conv-list">
          <p v-if="conversations.length === 0" class="conv-empty">{{ t('libraryChat.noHistory') }}</p>

          <div
            v-for="conv in conversations"
            :key="conv.id"
            class="conv-item"
            :class="{ active: activeConvId === conv.id }"
            @click="selectConversation(conv.id)"
          >
            <div class="conv-body">
              <div class="conv-title-text">{{ conv.title }}</div>
              <div class="conv-meta">
                <span>{{ formatDate(conv.updatedAt) }}</span>
                <span v-if="userMsgCount(conv) > 0" class="conv-turns">{{ userMsgCount(conv) }} 轮</span>
              </div>
            </div>
            <button
              class="conv-del-btn"
              :title="t('libraryChat.deleteConv')"
              @click.stop="deleteConversation(conv.id)"
            >
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>
        </div>
      </aside>

      <!-- ── Resize divider ────────────────────────────────────────────────────── -->
      <div class="chat-divider" @mousedown.prevent="onDividerMouseDown" />

      <!-- ── Main area ───────────────────────────────────────────────────────── -->
      <div class="lc-main">

        <!-- REMOVED: chat-header moved to lc-titlebar -->
        <div class="chat-header" style="display:none">
          <div class="tl-space" data-tauri-drag-region />
          <div class="header-left">
            <button class="sidebar-toggle-btn" @click="sidebarOpen = !sidebarOpen" :title="sidebarOpen ? '收起' : '展开'">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="9" y1="3" x2="9" y2="21"/>
              </svg>
            </button>
            <div class="header-avatar">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
                <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
                <path d="M8 9h8"/><path d="M8 13h5"/>
              </svg>
            </div>
            <div class="header-title-block">
              <span class="header-conv-title">{{ activeConv?.title || t('libraryChat.untitled') }}</span>
              <span class="header-subtitle">{{ conversationSubtitle }}</span>
            </div>
          </div>
          <div class="header-right">
            <!-- RAG not configured: show badge to open settings -->
            <button
              v-if="!ragStore.isConfigured"
              class="rag-badge inactive"
              title="点击配置 RAG"
              @click="emit('open-settings', 'rag')"
            >
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <ellipse cx="12" cy="5" rx="9" ry="3"/>
                <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/>
                <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
              </svg>
              RAG
            </button>

            <!-- RAG configured: show vectorization status + sync button -->
            <template v-else>
              <!-- Syncing progress (left of refresh button) -->
              <span v-if="syncingMissing" class="rag-sync-progress">
                {{ syncProgress.done }}/{{ syncProgress.total }}
              </span>

              <!-- Refresh button (moved to left) -->
              <button
                class="rag-refresh-btn"
                :class="{ refreshing: refreshingCounts || syncingMissing }"
                title="刷新嵌入状态"
                :disabled="refreshingCounts || syncingMissing"
                @click="refreshCounts"
              >
                <svg width="15" height="15" viewBox="-2 -2 28 28" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="23 4 23 10 17 10"/>
                  <path d="M20.49 15A9 9 0 1 1 23 10"/>
                </svg>
              </button>

              <div class="rag-counter" title="向量库：已嵌入论文 / 总论文数">
                <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                  <ellipse cx="12" cy="5" rx="9" ry="3"/>
                  <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/>
                  <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
                </svg>
                <span class="rag-counter-text">{{ vectorizedCount }}/{{ allPapers.length }}</span>
              </div>

              <!-- Syncing: cancel button -->
              <template v-if="syncingMissing">
                <button class="rag-sync-cancel" @click="syncCancelRequested = true" title="取消同步">
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
                  </svg>
                </button>
              </template>

              <!-- Sync missing / all-done button -->
              <button
                v-else
                class="rag-sync-btn"
                :class="{ 'all-done': unvectorizedPapers.length === 0 }"
                :title="unvectorizedPapers.length > 0 ? `嵌入 ${unvectorizedPapers.length} 篇未向量化的论文` : '所有论文已嵌入'"
                :disabled="unvectorizedPapers.length === 0"
                @click="syncMissing"
              >
                <svg v-if="unvectorizedPapers.length > 0" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <polyline points="16 16 12 12 8 16"/>
                  <line x1="12" y1="12" x2="12" y2="21"/>
                  <path d="M20.39 18.39A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.3"/>
                </svg>
                <svg v-else width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <polyline points="20 6 9 17 4 12"/>
                </svg>
                {{ unvectorizedPapers.length > 0 ? `嵌入 ${unvectorizedPapers.length} 篇` : '已全部嵌入' }}
              </button>
            </template>

            <div ref="modelMenuRoot" class="lc-model-picker">
              <button class="lc-model-trigger" @click.stop="modelMenuOpen = !modelMenuOpen">
                <span class="lc-model-icon">
                  <img
                    v-if="modelLogo(selectedModelOption)"
                    :src="modelLogo(selectedModelOption)"
                    alt=""
                  />
                  <span v-else>{{ selectedModelLabel().charAt(0).toUpperCase() }}</span>
                </span>
                <span class="lc-model-label">{{ selectedModelLabel() }}</span>
                <svg class="chevron" :class="{ open: modelMenuOpen }" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
                  <path d="m6 9 6 6 6-6"/>
                </svg>
              </button>

              <div v-if="modelMenuOpen" class="lc-model-menu">
                <div v-for="group in ai.groupedModels" :key="group.id" class="lc-model-group">
                  <div class="lc-model-group-name">{{ group.name }}</div>
                  <button
                    v-for="model in group.models"
                    :key="selectionKey(model)"
                    class="lc-model-row"
                    :class="{ active: selectionKey(model) === selectionKey(effectiveModel()) }"
                    @click="selectModel(model)"
                  >
                    <span class="lc-model-row-icon">
                      <img v-if="modelLogo(model)" :src="modelLogo(model)" alt="" />
                      <span v-else>{{ model.displayName.charAt(0).toUpperCase() }}</span>
                    </span>
                    <span class="lc-model-row-text">
                      <span class="lc-model-row-name">{{ model.displayName }}</span>
                      <span class="lc-model-row-meta">{{ modelCapabilityText(model) || model.modelId }}</span>
                    </span>
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- RAG hint banner -->
        <div v-if="ragStore.loaded && !ragStore.isConfigured" class="rag-hint-bar">
          <div class="rag-hint-icon">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="9"/><path d="M12 8v4"/><path d="M12 16h.01"/>
            </svg>
          </div>
          <span class="rag-hint-text">{{ t('libraryChat.ragHint') }}</span>
          <button class="rag-hint-action" @click="emit('open-settings', 'rag')">{{ t('libraryChat.ragHintAction') }}</button>
        </div>

        <!-- Messages -->
        <div ref="messagesEl" class="messages" @click="onMsgContainerClick">

          <!-- Empty state -->
          <div v-if="activeMessages.length === 0" class="empty-chat">
            <div class="empty-panel">
              <div class="empty-icon">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
                  <path d="M8 9h8"/><path d="M8 13h5"/>
                </svg>
              </div>
              <p class="empty-title">{{ t('libraryChat.title') }}</p>
              <p class="empty-hint">{{ t('libraryChat.placeholder') }}</p>
              <div class="empty-suggestions">
                <button
                  v-for="item in promptSuggestions"
                  :key="item"
                  class="suggestion-chip"
                  @click="useSuggestion(item)"
                >
                  {{ item }}
                </button>
              </div>
            </div>
          </div>

          <!-- Message list -->
          <template v-for="msg in activeMessages" :key="msg.id">

            <!-- User -->
            <div v-if="msg.role === 'user'" class="msg-row user">
              <div v-if="editingMsgId === msg.id" class="user-edit-card">
                <textarea
                  :id="`edit-${msg.id}`"
                  v-model="editingText"
                  class="user-edit-input"
                  rows="3"
                  @keydown.escape.prevent="cancelEdit"
                  @compositionstart="onCompositionStart"
                  @compositionend="onCompositionEnd"
                  @keydown.enter.exact.prevent="!isIMEActive() && submitUserEdit(msg)"
                />
                <div class="user-edit-actions">
                  <button class="edit-cancel" @click="cancelEdit">取消</button>
                  <button class="edit-submit" :disabled="!editingText.trim() || loading" @click="submitUserEdit(msg)">发送</button>
                </div>
              </div>
              <template v-else>
                <div class="user-bubble">{{ msg.content }}</div>
                <div class="message-actions user-actions">
                  <button :title="copiedMsgIds.has(msg.id) ? '已复制' : '复制'" @click="copyMessage(msg)">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                    </svg>
                  </button>
                  <button title="编辑并重发" :disabled="loading" @click="startEditUser(msg)">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>
                    </svg>
                  </button>
                </div>
              </template>
            </div>

            <!-- Assistant -->
            <div v-else class="msg-row assistant">
              <div class="assistant-wrap">
                <div class="assistant-avatar">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 2a2 2 0 0 1 2 2c0 .74-.4 1.39-1 1.73V7h1a7 7 0 0 1 7 7h1a1 1 0 0 1 1 1v3a1 1 0 0 1-1 1h-1v1a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-1H1a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1h1a7 7 0 0 1 7-7h1V5.73c-.6-.34-1-.99-1-1.73a2 2 0 0 1 2-2z"/>
                  </svg>
                </div>
                <div class="assistant-content">
                  <div class="message-actions assistant-actions">
                    <button :title="copiedMsgIds.has(msg.id) ? '已复制' : '复制'" @click="copyMessage(msg)">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                      </svg>
                    </button>
                    <button title="重新生成" :disabled="loading" @click="regenerateAssistant(msg, false)">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <polyline points="23 4 23 10 17 10"/><path d="M20.49 15A9 9 0 1 1 23 10"/>
                      </svg>
                    </button>
                    <button title="用右上角模型重答" :disabled="loading" @click="regenerateAssistant(msg, true)">
                      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M7 7h10"/><path d="M7 12h7"/><path d="M7 17h4"/><path d="m16 14 3 3-3 3"/>
                      </svg>
                    </button>
                  </div>

                  <div
                    class="assistant-bubble markdown-body"
                    :class="{ streaming: activeAnswer(msg).streaming, error: activeAnswer(msg).error }"
                  >
                    <!-- Streaming -->
                    <template v-if="activeAnswer(msg).streaming">
                      <div v-html="renderMarkdown(activeAnswer(msg).content || '<em style=\'opacity:.5\'>' + t('copilot.thinking') + '</em>')" />
                      <span class="cursor-blink"/>
                    </template>
                    <!-- Done: Mermaid-aware segment rendering -->
                    <template v-else>
                      <template v-for="(seg, si) in getSegments(activeAnswer(msg).content)" :key="si">
                        <div v-if="seg.type === 'md'" v-html="seg.html" />
                        <MermaidBlock v-else :src="seg.src" />
                      </template>
                    </template>
                  </div>

                  <div v-if="answerVariants(msg).length > 1" class="answer-tabs">
                    <button
                      v-for="(variant, index) in answerVariants(msg)"
                      :key="variant.id"
                      class="answer-tab"
                      :class="{ active: variant.id === msg.activeVariantId }"
                      @click="msg.activeVariantId = variant.id"
                    >
                      {{ variant.modelLabel || `回答 ${index + 1}` }}
                    </button>
                  </div>

                  <!-- Sources -->
                  <div v-if="!activeAnswer(msg).streaming && answerSources(msg).length > 0" class="sources-wrap">
                    <button class="sources-toggle" @click="toggleSources(msg.id)">
                      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <ellipse cx="12" cy="5" rx="9" ry="3"/>
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/>
                      </svg>
                      <span>{{ isSourcesExpanded(msg.id) ? t('libraryChat.hideSources') : t('libraryChat.sources', { n: groupedSources(answerSources(msg)).length }) }}</span>
                      <svg class="chevron" :class="{ open: isSourcesExpanded(msg.id) }" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <polyline points="6 9 12 15 18 9"/>
                      </svg>
                    </button>

                    <div v-if="isSourcesExpanded(msg.id)" class="sources-list">
                      <div v-for="group in groupedSources(answerSources(msg))" :key="group.paper_id" class="source-group">
                        <button class="source-paper-name" @click="openSourcePaper(group)">
                          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                            <polyline points="14 2 14 8 20 8"/>
                          </svg>
                          <span>{{ group.paper_title }}</span>
                        </button>
                        <div class="source-chips">
                          <span
                            v-for="chunk in group.chunks"
                            :key="chunk.chunk_id"
                            class="source-chip"
                            :class="`st-${chunk.source_type}`"
                          >
                            {{ sourceTypeLabel(chunk.source_type) }}
                            <span class="chip-score">{{ formatScore(chunk.score) }}</span>
                          </span>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>

          </template>
        </div>

        <!-- Input area -->
        <div class="input-area">
          <div class="composer">
            <textarea
              ref="textareaEl"
              v-model="input"
              class="chat-input"
              :placeholder="t('libraryChat.inputPlaceholder')"
              rows="1"
              :disabled="loading"
              @keydown="handleKeydown"
              @compositionstart="onCompositionStart"
              @compositionend="onCompositionEnd"
            />
            <div class="composer-footer">
              <div class="footer-left">
                <span class="rag-status" :class="{ on: ragStore.isConfigured }">
                  <span class="rag-dot" />
                  {{ ragStore.isConfigured ? 'RAG' : t('libraryChat.ragOff') }}
                </span>
              </div>
              <div class="footer-right">
                <span class="enter-hint">{{ t('libraryChat.enterHint') }}</span>
                <button class="send-btn" :disabled="!canSend" @click="sendMessage">
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/>
                  </svg>
                  {{ t('copilot.send') }}
                </button>
              </div>
            </div>
          </div>
        </div>

      </div><!-- /lc-main -->
      </div><!-- /lc-body -->
    </template>
  </div>
</template>

<style scoped>
/* ── Root layout ─────────────────────────────────────────────────────────── */

.lc-root {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: linear-gradient(180deg, var(--bg-primary), color-mix(in srgb, var(--bg-secondary) 54%, var(--bg-primary)));
}

/* ── Unified titlebar ─────────────────────────────────────────────────────── */

.lc-titlebar {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 44px;
  flex-shrink: 0;
  padding: 0 14px 0 0;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 85%, var(--bg-secondary));
}
.lc-titlebar .tl-space { width: 96px; flex-shrink: 0; }
.lc-titlebar-fill {
  flex: 1 1 auto;
  min-width: 12px;
}
.lc-titlebar-actions {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  flex: 0 0 auto;
  min-width: 0;
}
.lc-titlebar-actions button {
  margin: 0;
  -webkit-appearance: none;
  appearance: none;
}

/* ── Body (sidebar + main) ────────────────────────────────────────────────── */

.lc-body {
  display: flex;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

/* ── Sidebar ─────────────────────────────────────────────────────────────── */

.lc-sidebar {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: color-mix(in srgb, var(--bg-secondary) 82%, var(--bg-primary));
  overflow: hidden;
}

.chat-divider {
  width: 1px;
  flex-shrink: 0;
  background: transparent;
  cursor: col-resize;
  position: relative;
}
.chat-divider::before {
  content: '';
  position: absolute;
  top: 0; bottom: 0;
  left: 50%;
  width: 1px;
  transform: translateX(-50%);
  background: var(--border-default);
  transition: width 0.12s ease, background 0.12s ease;
}
.chat-divider::after {
  content: '';
  position: absolute;
  top: 0; bottom: 0;
  left: -3px; right: -3px;
}
.chat-divider:hover::before {
  width: 3px;
  background: var(--accent);
}

.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 12px 10px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.sidebar-heading {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}

.sidebar-title {
  font-size: 13px;
  font-weight: 700;
  color: var(--text-secondary);
  white-space: nowrap;
}

.sidebar-count {
  min-width: 20px;
  height: 18px;
  padding: 0 6px;
  border-radius: var(--radius-pill);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  font-size: 11px;
  font-weight: 650;
}

.new-chat-btn {
  height: 30px;
  padding: 0 10px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  border-radius: var(--radius-md);
  border: 1px solid color-mix(in srgb, var(--accent) 28%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 7%, var(--bg-primary));
  color: var(--accent);
  font-size: 12px;
  font-weight: 650;
  flex-shrink: 0;
}

.new-chat-btn:hover {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 45%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 11%, var(--bg-primary));
}

.conv-list {
  flex: 1;
  overflow-y: auto;
  padding: 10px 8px;
}

.conv-empty {
  padding: 14px 10px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  text-align: center;
}

.conv-item {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 10px 10px 10px 12px;
  border-radius: var(--radius-md);
  cursor: pointer;
  position: relative;
  transition: background 0.12s, box-shadow 0.12s;
  min-width: 0;
  border: 1px solid transparent;
}

.conv-item:hover { background: var(--bg-hover); }
.conv-item.active {
  background: color-mix(in srgb, var(--accent) 12%, var(--bg-primary));
  border-color: color-mix(in srgb, var(--accent) 16%, transparent);
}
.conv-item.active .conv-title-text { color: var(--accent); }

.conv-body { flex: 1; min-width: 0; }

.conv-title-text {
  font-size: 13px;
  font-weight: 650;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  line-height: 1.4;
}

.conv-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--text-tertiary);
  margin-top: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.conv-turns {
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  background: var(--bg-tertiary);
  color: var(--text-secondary);
  font-weight: 600;
}

.conv-del-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  flex-shrink: 0;
  opacity: 0;
  transition: opacity 0.1s;
  margin-top: 1px;
}

.conv-item:hover .conv-del-btn { opacity: 1; }
.conv-del-btn:hover { background: color-mix(in srgb, #cc3333 12%, transparent); color: #cc3333; }

/* ── Main area ───────────────────────────────────────────────────────────── */

.lc-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* ── Header ──────────────────────────────────────────────────────────────── */

/* .chat-header is now hidden (content moved to lc-titlebar) */
.chat-header { display: none !important; }

.header-left {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
  flex: 1;
}

.sidebar-toggle-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.sidebar-toggle-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.header-avatar {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  flex-shrink: 0;
}

.header-title-block {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.header-conv-title {
  font-size: 14px;
  font-weight: 650;
  letter-spacing: -0.01em;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.header-subtitle {
  font-size: 11px;
  color: var(--text-tertiary);
  background: var(--bg-tertiary);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
  flex-shrink: 0;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.lc-model-picker {
  position: relative;
  flex-shrink: 0;
}

.lc-model-trigger {
  height: 32px;
  min-width: 190px;
  max-width: 260px;
  padding: 0 10px 0 8px;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-primary);
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.03);
}

.lc-model-trigger:hover {
  border-color: color-mix(in srgb, var(--accent) 32%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 4%, var(--bg-primary));
}

.lc-model-icon,
.lc-model-row-icon {
  width: 22px;
  height: 22px;
  border-radius: 7px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  overflow: hidden;
  color: var(--accent);
  font-size: 11px;
  font-weight: 700;
}

.lc-model-icon img,
.lc-model-row-icon img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.lc-model-label {
  min-width: 0;
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
  font-size: 13px;
  font-weight: 650;
}

.lc-model-trigger .chevron {
  color: var(--text-tertiary);
  transition: transform 0.14s ease;
}

.lc-model-trigger .chevron.open { transform: rotate(180deg); }

.lc-model-menu {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 30;
  width: 300px;
  max-height: min(460px, 70vh);
  overflow-y: auto;
  padding: 8px;
  border: 1px solid var(--border-subtle);
  border-radius: 14px;
  background: color-mix(in srgb, var(--bg-primary) 96%, var(--bg-secondary));
  box-shadow: 0 20px 50px rgba(15, 23, 42, 0.18);
}

.lc-model-group + .lc-model-group {
  margin-top: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--border-subtle);
}

.lc-model-group-name {
  padding: 3px 8px 6px;
  color: var(--text-tertiary);
  font-size: 11px;
  font-weight: 700;
}

.lc-model-row {
  width: 100%;
  min-height: 42px;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 7px 8px;
  border-radius: 10px;
  color: var(--text-secondary);
  text-align: left;
}

.lc-model-row:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.lc-model-row.active {
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
}

.lc-model-row-text {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.lc-model-row-name {
  font-size: 13px;
  font-weight: 700;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.lc-model-row-meta {
  font-size: 11px;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rag-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 11px;
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 650;
  letter-spacing: 0;
  cursor: pointer;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
  flex-shrink: 0;
}
.rag-badge.inactive {
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  border: 1px solid var(--border-subtle);
}
.rag-badge.inactive:hover { background: var(--bg-hover); }

/* Vectorized count display */
.rag-counter {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  font-size: 11px;
  font-weight: 600;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  flex-shrink: 0;
  user-select: none;
}
.rag-counter-text { letter-spacing: 0.2px; }

/* Sync missing button */
.rag-sync-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 10px;
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.rag-sync-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* All-done state for sync button */
.rag-sync-btn.all-done {
  color: var(--text-tertiary);
  border-color: var(--border-subtle);
  cursor: default;
  opacity: 0.7;
}
.rag-sync-btn.all-done:disabled {
  opacity: 0.7;
  cursor: default;
}

/* Refresh button */
.rag-refresh-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  min-width: 32px;
  height: 32px;
  padding: 0;
  line-height: 0;
  overflow: visible;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.rag-refresh-btn svg {
  display: block;
  flex-shrink: 0;
  overflow: visible;
}
.rag-refresh-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.rag-refresh-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.rag-refresh-btn.refreshing svg {
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

/* Syncing progress */
.rag-sync-progress {
  font-size: 12px;
  font-weight: 500;
  color: var(--accent);
  min-width: 36px;
  text-align: center;
}
.rag-sync-cancel {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  cursor: pointer;
  flex-shrink: 0;
}
.rag-sync-cancel:hover { background: var(--bg-hover); color: var(--text-primary); }

/* ── RAG hint bar ────────────────────────────────────────────────────────── */

.rag-hint-bar {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 40px;
  padding: 0 22px;
  font-size: 12px;
  color: var(--text-secondary);
  background: color-mix(in srgb, #f59e0b 6%, var(--bg-primary));
  border-bottom: 1px solid color-mix(in srgb, #f59e0b 18%, var(--border-subtle));
  flex-shrink: 0;
}

.rag-hint-icon {
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-pill);
  color: #d88a00;
  background: color-mix(in srgb, #f59e0b 12%, transparent);
  flex-shrink: 0;
}

.rag-hint-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rag-hint-action {
  font-size: 12px;
  font-weight: 600;
  color: var(--accent);
  white-space: nowrap;
  flex-shrink: 0;
  margin-left: auto;
}

.rag-hint-action:hover { text-decoration: underline; }

/* ── Messages ────────────────────────────────────────────────────────────── */

.messages {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 24px 32px;
  display: flex;
  flex-direction: column;
  gap: 22px;
}

/* ── Empty state ─────────────────────────────────────────────────────────── */

.empty-chat {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  text-align: center;
  padding: 32px 20px 56px;
}

.empty-panel {
  width: min(640px, 100%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.empty-icon {
  width: 64px;
  height: 64px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 18px;
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-secondary));
  color: var(--accent);
  border: 1px solid color-mix(in srgb, var(--accent) 18%, transparent);
}

.empty-title {
  margin-top: 2px;
  font-size: 22px;
  font-weight: 750;
  color: var(--text-primary);
}

.empty-hint {
  font-size: 13px;
  color: var(--text-tertiary);
  max-width: 420px;
  line-height: 1.5;
}

.empty-suggestions {
  width: min(560px, 100%);
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  margin-top: 8px;
}

.suggestion-chip {
  min-height: 42px;
  padding: 0 14px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 13px;
  font-weight: 550;
  text-align: left;
  transition: border-color 0.12s, background 0.12s, color 0.12s, transform 0.12s;
}

.suggestion-chip:hover {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 32%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 6%, var(--bg-primary));
  transform: translateY(-1px);
}

/* ── Message rows ────────────────────────────────────────────────────────── */

.msg-row {
  width: 100%;
  margin: 0;
  position: relative;
}

/* User message */
.msg-row.user {
  display: flex;
  justify-content: flex-end;
  align-items: flex-start;
  gap: 8px;
}

.user-bubble {
  max-width: min(76%, 680px);
  padding: 11px 15px;
  background: color-mix(in srgb, var(--accent) 92%, #ffffff);
  color: #fff;
  border-radius: 16px;
  border-bottom-right-radius: 4px;
  font-size: 14px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  box-shadow: 0 6px 18px color-mix(in srgb, var(--accent) 18%, transparent);
}

/* Assistant message */
.msg-row.assistant { display: flex; }

.assistant-wrap {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  max-width: 100%;
  min-width: 0;
}

.assistant-avatar {
  width: 30px;
  height: 30px;
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  margin-top: 2px;
  border: 1px solid color-mix(in srgb, var(--accent) 20%, transparent);
}

.assistant-content {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.assistant-bubble {
  padding: 2px 0;
  background: transparent;
  border: none;
  border-radius: 0;
  font-size: 14px;
  line-height: 1.72;
  color: var(--text-primary);
  word-break: break-word;
  overflow-x: auto;
}

.assistant-bubble.streaming { opacity: 0.88; }

.assistant-bubble.error {
  padding: 12px 14px;
  background: color-mix(in srgb, #cc3333 8%, transparent);
  border: 1px solid color-mix(in srgb, #cc3333 25%, transparent);
  border-radius: var(--radius-md);
  color: #cc3333;
}

.message-actions {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s ease;
}

.msg-row:hover .message-actions,
.message-actions:focus-within {
  opacity: 1;
  pointer-events: auto;
}

.message-actions button {
  width: 27px;
  height: 27px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 92%, var(--bg-secondary));
  color: var(--text-tertiary);
  box-shadow: 0 6px 18px rgba(15, 23, 42, 0.08);
}

.message-actions button:hover:not(:disabled) {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 28%, var(--border-subtle));
  background: color-mix(in srgb, var(--accent) 7%, var(--bg-primary));
}

.message-actions button:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}

.user-actions {
  order: -1;
  margin-top: 4px;
}

.assistant-actions {
  position: absolute;
  top: -30px;
  left: 0;
  z-index: 2;
}

.user-edit-card {
  width: min(680px, 78%);
  padding: 10px;
  border: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border-default));
  border-radius: 16px;
  background: var(--bg-primary);
  box-shadow: 0 10px 26px rgba(15, 23, 42, 0.09);
}

.user-edit-input {
  width: 100%;
  min-height: 96px;
  resize: vertical;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font: inherit;
  line-height: 1.6;
}

.user-edit-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 8px;
}

.edit-cancel,
.edit-submit {
  height: 30px;
  padding: 0 12px;
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 650;
}

.edit-cancel {
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
}

.edit-submit {
  color: #fff;
  background: var(--accent);
}

.edit-submit:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}

.answer-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 2px;
}

.answer-tab {
  max-width: 180px;
  height: 25px;
  padding: 0 9px;
  border-radius: var(--radius-pill);
  border: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  color: var(--text-tertiary);
  font-size: 11px;
  font-weight: 650;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.answer-tab.active {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 28%, var(--border-subtle));
  background: color-mix(in srgb, var(--accent) 9%, var(--bg-primary));
}

/* Streaming cursor */
.cursor-blink {
  display: inline-block;
  width: 2px;
  height: 1em;
  background: var(--accent);
  border-radius: 1px;
  margin-left: 2px;
  vertical-align: text-bottom;
  animation: blink 1s step-end infinite;
}

@keyframes blink { 0%, 100% { opacity: 1; } 50% { opacity: 0; } }

/* ── Sources ─────────────────────────────────────────────────────────────── */

.sources-wrap {
  display: flex;
  flex-direction: column;
  gap: 0;
}

.sources-toggle {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 9px;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--bg-secondary) 70%, var(--bg-primary));
  cursor: pointer;
  align-self: flex-start;
  transition: color 0.1s, border-color 0.1s;
}

.sources-toggle:hover {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 40%, transparent);
}

.sources-toggle .chevron {
  transition: transform 0.18s ease;
}

.sources-toggle .chevron.open { transform: rotate(180deg); }

.sources-list {
  margin-top: 6px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 11px 12px;
  background: color-mix(in srgb, var(--bg-secondary) 62%, var(--bg-primary));
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
}

.source-group {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.source-paper-name {
  display: flex;
  align-items: center;
  gap: 5px;
  max-width: 100%;
  font-size: 11px;
  font-weight: 500;
  color: var(--text-secondary);
  overflow: hidden;
  text-align: left;
  border-radius: var(--radius-sm);
}

.source-paper-name svg { flex-shrink: 0; }

.source-paper-name:hover {
  color: var(--accent);
}

.source-paper-name span {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.source-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.source-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 7px;
  border-radius: var(--radius-pill);
  font-size: 10px;
  font-weight: 600;
  white-space: nowrap;
}

.chip-score {
  font-weight: 400;
  opacity: 0.75;
}

.st-metadata {
  background: color-mix(in srgb, #4a90d9 14%, transparent);
  color: #4a90d9;
}

.st-text {
  background: var(--bg-tertiary);
  color: var(--text-secondary);
}

.st-highlight {
  background: color-mix(in srgb, #e0a200 14%, transparent);
  color: #c08a00;
}

.st-note {
  background: color-mix(in srgb, #2da44e 14%, transparent);
  color: #2da44e;
}

/* ── Input area ──────────────────────────────────────────────────────────── */

.input-area {
  flex-shrink: 0;
  border-top: 1px solid var(--border-subtle);
  padding: 14px 24px 18px;
  background: color-mix(in srgb, var(--bg-secondary) 78%, var(--bg-primary));
}

.composer {
  width: 100%;
  margin: 0 auto;
  padding: 12px 14px 10px;
  border: 1px solid var(--border-default);
  border-radius: 16px;
  background: var(--bg-primary);
  box-shadow: 0 10px 28px rgba(15, 23, 42, 0.07);
  transition: border-color 0.15s;
}

.composer:focus-within {
  border-color: color-mix(in srgb, var(--accent) 50%, var(--border-default));
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 10%, transparent);
}

.chat-input {
  width: 100%;
  min-height: 50px;
  max-height: 160px;
  padding: 2px 0 6px;
  font-size: 14px;
  border: 0;
  background: transparent;
  color: var(--text-primary);
  resize: none;
  line-height: 1.6;
  font-family: inherit;
  overflow-y: auto;
}

.chat-input:focus { outline: none; }
.chat-input:disabled { opacity: 0.5; }

.composer-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 6px;
}

.footer-left { display: flex; align-items: center; gap: 8px; }
.footer-right { display: flex; align-items: center; gap: 8px; }

.rag-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 8px;
  border-radius: var(--radius-pill);
  background: var(--bg-secondary);
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
}

.rag-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-tertiary);
  flex-shrink: 0;
}

.rag-status.on { color: var(--accent); }
.rag-status.on .rag-dot { background: var(--accent); }

.enter-hint {
  font-size: 11px;
  color: var(--text-tertiary);
}

.send-btn {
  height: 34px;
  padding: 0 15px;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-lg);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  font-size: 13px;
  font-weight: 650;
  flex-shrink: 0;
}

.send-btn:hover:not(:disabled) { background: var(--accent-hover); }
.send-btn:disabled { opacity: 0.38; cursor: not-allowed; }

/* ── No-AI hint ──────────────────────────────────────────────────────────── */

.center-hint {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 40px;
  text-align: center;
}

.hint-icon {
  width: 48px;
  height: 48px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-lg);
  background: var(--bg-secondary);
  color: var(--text-tertiary);
  margin-bottom: 4px;
}

.hint-title {
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
}

.hint-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  max-width: 260px;
}

.btn-primary {
  padding: 7px 16px;
  font-size: var(--font-size-sm);
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-md);
}

.btn-primary:hover { background: var(--accent-hover); }

/* ── Markdown body overrides ─────────────────────────────────────────────── */

.markdown-body :deep(h1),
.markdown-body :deep(h2),
.markdown-body :deep(h3) {
  margin-top: 1em;
  margin-bottom: 0.4em;
  font-weight: 600;
  line-height: 1.35;
}

.markdown-body :deep(p) { margin: 0.45em 0; }
.markdown-body :deep(ul),
.markdown-body :deep(ol) { padding-left: 1.4em; margin: 0.5em 0; }
.markdown-body :deep(li) { margin: 0.2em 0; }
.markdown-body :deep(blockquote) {
  border-left: 3px solid var(--accent);
  margin: 0.6em 0;
  padding: 4px 0 4px 12px;
  color: var(--text-secondary);
}

.markdown-body :deep(table) {
  border-collapse: collapse;
  width: 100%;
  font-size: 0.92em;
  margin: 0.7em 0;
}

.markdown-body :deep(th),
.markdown-body :deep(td) {
  border: 1px solid var(--border-default);
  padding: 5px 10px;
  text-align: left;
}

.markdown-body :deep(th) {
  background: var(--bg-tertiary);
  font-weight: 600;
}

.markdown-body :deep(.md-code-block) {
  margin: 0.6em 0;
  border-radius: var(--radius-md);
  overflow: hidden;
  font-size: 0.88em;
}

.markdown-body :deep(.katex-display) {
  overflow-x: auto;
  overflow-y: hidden;
  margin: 0.6em 0;
}

@media (max-width: 860px) {
  .lc-sidebar { min-width: 160px; }
  .new-chat-btn span { display: none; }
  .header-avatar { display: none; }
  .lc-model-trigger { min-width: 160px; max-width: 190px; }
  .lc-model-menu { width: 280px; }
  .empty-suggestions { grid-template-columns: 1fr; }
  .enter-hint { display: none; }
}

/* ── Scrollbar ───────────────────────────────────────────────────────────── */

.messages::-webkit-scrollbar,
.conv-list::-webkit-scrollbar { width: 4px; }

.messages::-webkit-scrollbar-track,
.conv-list::-webkit-scrollbar-track { background: transparent; }

.messages::-webkit-scrollbar-thumb,
.conv-list::-webkit-scrollbar-thumb {
  background: var(--border-default);
  border-radius: 2px;
}
</style>
