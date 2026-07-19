<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useAiStore, type ModelOption } from '../stores/ai'
import { useRagStore } from '../stores/rag'
import { useSettingsStore } from '../stores/settings'
import MermaidBlock from './MermaidBlock.vue'
import WindowControls from './WindowControls.vue'
import { renderMarkdown, getSegments } from '../utils/renderMarkdown'
import { svgStringToPngBlob } from '../utils/svgToPng'
import { copyPngBlobToClipboard } from '../utils/clipboard'
import { buildChunks } from '../utils/chunker'
import { sortPapersByRecentAccess } from '../utils/recentPapers'
import type { ChatContentPart, ChatMessage, ModelSelection, RetrievedChunk, PaperIndexEntry, PaperVectorizeInput, ChunkInput } from '../types'

const emit = defineEmits<{ 'open-settings': [section?: 'ai' | 'rag'] }>()
const { t } = useI18n()
// On Windows the native decorations are off, so we drop the macOS traffic-light
// gutter and render our own window controls (see WindowControls).
const isWindows = navigator.userAgent.toLowerCase().includes('windows')
const ai = useAiStore()
const ragStore = useRagStore()
const settingsStore = useSettingsStore()

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
  const t0 = Date.now()
  try {
    await invoke('sync_vectorized_flags')
    await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts(), loadSnippetStoreCounts()])
  } finally {
    const remaining = 700 - (Date.now() - t0)
    if (remaining > 0) await new Promise(r => setTimeout(r, remaining))
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
  // Small worker pool — embedding API latency dominates, so a few papers
  // in flight at once give a near-linear speedup.
  const CONCURRENCY = 3
  const queue = [...papers]
  const workers = Array.from({ length: Math.min(CONCURRENCY, queue.length) }, async () => {
    while (!syncCancelRequested) {
      const paper = queue.shift()
      if (!paper) break
      try {
        const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: paper.slug })
        const chunks: ChunkInput[] = await buildChunks(input, s.chunk_size ?? 512, s.chunk_overlap ?? 50)
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
  })
  await Promise.all(workers)
  syncingMissing.value = false
  emitTo('main', 'rag-embed-progress', { syncing: false, done, total: papers.length }).catch(() => {})
  await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts()])
}

// ── Types ─────────────────────────────────────────────────────────────────────

interface LibraryAnswerVariant {
  id: string
  content: string
  // Throttled copy of `content` used for live markdown rendering while streaming
  // (re-rendering the full markdown on every token freezes the UI on long answers).
  displayContent?: string
  sources?: RetrievedChunk[]
  streaming?: boolean
  error?: boolean
  createdAt: string
  model?: ModelSelection | null
  modelLabel?: string
  // Accumulated thinking/reasoning tokens (shown in the collapsible 思考过程 box)
  // when reasoning mode was on for this turn.
  reasoningContent?: string
  contextContent?: LibrarySentContextPayload
  // Titles of the papers sent as context on this turn. Kept when persisting (the
  // heavy `contextContent` is stripped) so the per-message badge + dedup survive
  // a reload — mirrors how AiTab persists its context flags.
  contextPaperLabels?: string[]
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  costUsd?: number | null
  startedAt?: number
  endedAt?: number
}

interface LibraryUiMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  displayContent?: string
  attachments?: Attachment[]
  sources?: RetrievedChunk[]
  streaming?: boolean
  error?: boolean
  createdAt: string
  model?: ModelSelection | null
  modelLabel?: string
  variants?: LibraryAnswerVariant[]
  activeVariantId?: string
  reasoningContent?: string
  contextContent?: LibrarySentContextPayload
  contextPaperLabels?: string[]
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  costUsd?: number | null
  startedAt?: number
  endedAt?: number
}

interface LibraryConversation {
  id: string
  title: string
  messages: LibraryUiMessage[]
  selectedPaperSlugs: string[]
  createdAt: string
  updatedAt: string
}

interface GroupedSource {
  paper_id: string
  paper_title: string
  slug: string
  chunks: RetrievedChunk[]
}

interface LibrarySentContextSection {
  kind?: string
  label: string
  content: string
}

interface LibrarySentContextPayload {
  mode?: string
  sections?: LibrarySentContextSection[]
}

interface Attachment {
  id: string
  type: 'image' | 'pdf'
  name: string
  dataUrl: string
}

interface StreamUsagePayload {
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cost_usd?: number | null
}

// ── Storage ───────────────────────────────────────────────────────────────────

const STORAGE_KEY = 'argus.library-chats.v1'
const LAST_MODEL_KEY = 'argus.library-chat.last-model'
const KNOWLEDGE_SOURCE_KEY = 'argus.library-chat.knowledge-source.v2'

function genId() {
  return Date.now().toString(36) + Math.random().toString(36).slice(2, 7)
}

function normalizeSelectedPaperSlugs(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return [...new Set(value.filter((v): v is string => typeof v === 'string'))].slice(0, 50)
}

function normalizeConversations(parsed: unknown): LibraryConversation[] {
  if (!Array.isArray(parsed)) return []
  return parsed
    .filter((conv): conv is Partial<LibraryConversation> =>
      !!conv &&
      typeof conv === 'object' &&
      typeof conv.id === 'string' &&
      Array.isArray(conv.messages)
    )
    .map(conv => ({
      id: conv.id!,
      title: conv.title || t('libraryChat.untitled'),
      messages: conv.messages!,
      selectedPaperSlugs: normalizeSelectedPaperSlugs(conv.selectedPaperSlugs),
      createdAt: conv.createdAt || new Date().toISOString(),
      updatedAt: conv.updatedAt || conv.createdAt || new Date().toISOString(),
    }))
}

// Legacy conversations lived in a single global localStorage key, so they were
// shared across every library (data bleed). Read them once for migration.
function readLegacyLocalConversations(): LibraryConversation[] {
  try {
    return normalizeConversations(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]'))
  } catch {
    return []
  }
}

// Load conversations for the currently-open library from the library folder
// (`.argus/library_chats.json`). On first run after upgrade, migrate any
// conversations left in the old global localStorage cache into this library,
// then clear the legacy key so it never re-imports into a second library.
async function loadConversations(): Promise<LibraryConversation[]> {
  let convs: LibraryConversation[] = []
  try {
    convs = normalizeConversations(await invoke<unknown>('get_library_conversations'))
  } catch {
    convs = []
  }
  const legacy = readLegacyLocalConversations()
  if (legacy.length > 0) {
    try { localStorage.removeItem(STORAGE_KEY) } catch {}
    if (convs.length === 0) {
      convs = legacy
      persistConversations(convs)
    }
  }
  return convs
}

function stripTransientContext(msg: LibraryUiMessage): LibraryUiMessage {
  const clone: LibraryUiMessage = {
    ...msg,
    variants: msg.variants?.map(variant => {
      const variantClone: LibraryAnswerVariant = { ...variant }
      delete variantClone.contextContent
      delete variantClone.displayContent
      return variantClone
    }),
  }
  delete clone.contextContent
  delete clone.displayContent
  return clone
}

function persistConversations(convs: LibraryConversation[]) {
  const serializable = convs.slice(0, 50).map(conv => ({
    ...conv,
    selectedPaperSlugs: normalizeSelectedPaperSlugs(conv.selectedPaperSlugs),
    messages: conv.messages.map(stripTransientContext),
  }))
  // Fire-and-forget: persisted into the active library's `.argus/` folder.
  invoke('save_library_conversations', { conversations: serializable }).catch(() => {})
}

// ── State ─────────────────────────────────────────────────────────────────────

const conversations = ref<LibraryConversation[]>([])
const activeConvId = ref<string | null>(null)
const input = ref('')
const attachments = ref<Attachment[]>([])
const fileInputRef = ref<HTMLInputElement | null>(null)
const previewImage = ref<string | null>(null)
const previewPdf = ref<string | null>(null)
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
const modelPickerMsgId = ref<string | null>(null)
const modelPickerPos = ref<{ top: number; left: number }>({ top: 0, left: 0 })
const expandedContextId = ref<string | null>(null)

const modelPickerMsg = computed(() =>
  modelPickerMsgId.value
    ? (activeConv.value?.messages.find(m => m.id === modelPickerMsgId.value) as LibraryUiMessage | undefined)
    : undefined
)

// ── Knowledge source picker ───────────────────────────────────────────────────
type KnowledgeSource = 'paper-rag' | 'papers' | 'snippets'

function loadKnowledgeSource(): KnowledgeSource {
  const saved = localStorage.getItem(KNOWLEDGE_SOURCE_KEY)
  if (saved === 'papers' || saved === 'paper-rag' || saved === 'snippets') return saved
  return 'paper-rag'
}

const knowledgeSource = ref<KnowledgeSource>(loadKnowledgeSource())
const sourcePickerOpen = ref(false)

// Reasoning / thinking-mode state (mirrors AiTab). DeepSeek exposes high/max;
// everyone else low/medium/high — the backend maps DeepSeek's levels.
const useReasoning = ref(false)
const reasoningLevel = ref<'low' | 'medium' | 'high'>('high')
const reasoningOpen = ref(false)
const isDeepSeekSelected = computed(() => {
  const sel = effectiveModel()
  if (!sel) return false
  const provider = ai.settings.providers.find(p => p.id === sel.providerId)
  return !!provider?.base_url.toLowerCase().includes('deepseek')
})

// Per-answer "思考过程" collapse state. Absence = expanded (the default, so the
// user sees the model think); presence = the user collapsed it. Keyed by answer id.
const collapsedReasoning = ref<Set<string>>(new Set())
function isReasoningCollapsed(id: string) {
  return collapsedReasoning.value.has(id)
}
function toggleReasoning(id: string) {
  const next = new Set(collapsedReasoning.value)
  next.has(id) ? next.delete(id) : next.add(id)
  collapsedReasoning.value = next
}
// "126 词 · 147 字符" — CJK chars count as one word each, Latin runs as one word.
function reasoningStats(text: string) {
  const chars = text.length
  const cjk = (text.match(/[一-鿿぀-ヿ가-힯]/g) ?? []).length
  const latin = (text.match(/[A-Za-z0-9]+/g) ?? []).length
  return `${cjk + latin} 词 · ${chars} 字符`
}

const paperPickerOpen = ref(false)
const paperPickerSearch = ref('')
const selectedPaperSlugs = computed(() => {
  const conv = conversations.value.find(c => c.id === activeConvId.value)
  return conv?.selectedPaperSlugs ?? []
})

function setKnowledgeSource(src: KnowledgeSource) {
  knowledgeSource.value = src
  sourcePickerOpen.value = false
  try { localStorage.setItem(KNOWLEDGE_SOURCE_KEY, src) } catch {}
}

const knowledgeSourceLabel = computed(() =>
  knowledgeSource.value === 'snippets'
    ? '素材库'
    : knowledgeSource.value === 'paper-rag'
      ? '文献库RAG'
      : '文献库'
)

function setActiveSelectedPaperSlugs(slugs: string[]) {
  const conv = conversations.value.find(c => c.id === activeConvId.value)
  if (!conv) return
  conv.selectedPaperSlugs = normalizeSelectedPaperSlugs(slugs)
  persistConversations(conversations.value)
}

const selectedPapers = computed(() => {
  const bySlug = new Map(allPapers.value.map(p => [p.slug, p]))
  return selectedPaperSlugs.value.map(slug => bySlug.get(slug)).filter((p): p is PaperIndexEntry => !!p)
})

const pickerPapers = computed(() => {
  const q = paperPickerSearch.value.trim().toLowerCase()
  const papers = sortPapersByRecentAccess(allPapers.value)
  if (!q) return papers
  return papers.filter(p =>
    p.title.toLowerCase().includes(q) ||
    p.authors.some(a => a.toLowerCase().includes(q)) ||
    String(p.year ?? '').includes(q)
  )
})

function openPaperPicker() {
  paperPickerSearch.value = ''
  paperPickerOpen.value = true
}

function addSelectedPaper(paper: PaperIndexEntry) {
  // Adding a paper to the chat context isn't "reading" — don't touch recency.
  if (!selectedPaperSlugs.value.includes(paper.slug)) {
    setActiveSelectedPaperSlugs([...selectedPaperSlugs.value, paper.slug])
  }
}

function removeSelectedPaper(slug: string) {
  setActiveSelectedPaperSlugs(selectedPaperSlugs.value.filter(s => s !== slug))
}

// The picker is now the only place to manage the selection (the above-input strip
// was removed), so a click there toggles a paper in or out.
function toggleSelectedPaper(paper: PaperIndexEntry) {
  if (selectedPaperSlugs.value.includes(paper.slug)) {
    removeSelectedPaper(paper.slug)
  } else {
    addSelectedPaper(paper)
  }
}

function clearSelectedPapers() {
  setActiveSelectedPaperSlugs([])
}

// ── Snippet store state ───────────────────────────────────────────────────────
const snippetEmbeddedCount  = ref(0)
const snippetTotalCount     = ref(0)
const snippetSyncing        = ref(false)
const snippetSyncProgress   = ref({ done: 0, total: 0, failed: 0 })
let   snippetSyncCancel     = false

async function loadSnippetStoreCounts() {
  try {
    const [info, allLibs] = await Promise.all([
      invoke<{ embedded_count: number }>('get_snippet_store_info'),
      invoke<{ id: string }[]>('list_snippet_libraries'),
    ])
    snippetEmbeddedCount.value = info.embedded_count

    // Count total snippets across all libraries
    let total = 0
    for (const lib of allLibs) {
      const snips = await invoke<unknown[]>('get_snippets', { libraryId: lib.id })
      total += snips.length
    }
    snippetTotalCount.value = total
  } catch { /* no library open */ }
}

async function syncSnippets() {
  if (snippetSyncing.value || !ragStore.isConfigured) return
  snippetSyncing.value = true
  snippetSyncCancel = false
  snippetSyncProgress.value = { done: 0, total: snippetTotalCount.value - snippetEmbeddedCount.value, failed: 0 }
  // Live progress from the backend while batches are embedded
  const unlistenProgress = await listen<{ done: number; failed: number; total: number }>(
    'snippet-embed-progress',
    (ev) => {
      snippetSyncProgress.value = {
        done: ev.payload.done,
        total: ev.payload.total,
        failed: ev.payload.failed,
      }
    },
  )
  try {
    const [done, failed] = await invoke<[number, number]>('embed_all_snippets')
    snippetSyncProgress.value = { done, total: done + failed, failed }
    await loadSnippetStoreCounts()
  } catch { /* ignore */ }
  finally {
    unlistenProgress()
    snippetSyncing.value = false
  }
}

function openModelPicker(msgId: string, e: MouseEvent) {
  if (modelPickerMsgId.value === msgId) {
    modelPickerMsgId.value = null
    return
  }
  const rect = (e.currentTarget as HTMLElement).getBoundingClientRect()
  // Store the top of the button; popup uses translateY(-100%) to appear above it
  modelPickerPos.value = { top: rect.top - 6, left: rect.left }
  modelPickerMsgId.value = msgId
}

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

// Per-request event unlisteners, keyed by the streaming target's id, so
// concurrent / rapid requests never overwrite each other's listeners (the old
// module-level singletons leaked and cross-contaminated messages).
const activeUnlisteners = new Map<string, UnlistenFn[]>()
// Targets the user explicitly stopped — used to block a late `finalText`
// from refilling content after the front-end has already halted streaming.
const stoppedTargetIds = new Set<string>()
// Maps a streaming target id -> the backend request_id we sent, so stopStreaming
// can tell the backend to truly cancel the in-flight HTTP request (stop billing).
const activeRequestIds = new Map<string, string>()

// ── Throttled streaming render ────────────────────────────────────────────────
// Re-rendering full markdown (markdown-it + KaTeX + highlight.js) on every
// streamed token is O(n²) and freezes the UI on long answers. We refresh a
// `displayContent` copy at most once per STREAM_RENDER_MS instead.
const STREAM_RENDER_MS = 90
const streamRenderTimers = new Map<string, ReturnType<typeof setTimeout>>()
const streamRenderLast = new Map<string, number>()

type StreamTarget = LibraryUiMessage | LibraryAnswerVariant

function scheduleStreamRender(target: StreamTarget) {
  const now = Date.now()
  const last = streamRenderLast.get(target.id) ?? 0
  const elapsed = now - last
  if (elapsed >= STREAM_RENDER_MS) {
    streamRenderLast.set(target.id, now)
    target.displayContent = target.content
    scrollToBottom()
    return
  }
  if (streamRenderTimers.has(target.id)) return
  const timer = setTimeout(() => {
    streamRenderTimers.delete(target.id)
    streamRenderLast.set(target.id, Date.now())
    target.displayContent = target.content
    scrollToBottom()
  }, STREAM_RENDER_MS - elapsed)
  streamRenderTimers.set(target.id, timer)
}

// Final flush so the last tokens render even if a throttle window was pending.
function flushStreamRender(target: StreamTarget) {
  const timer = streamRenderTimers.get(target.id)
  if (timer) { clearTimeout(timer); streamRenderTimers.delete(target.id) }
  streamRenderLast.delete(target.id)
  target.displayContent = target.content
}

// Clear every pending throttle timer (session switch / unmount).
function clearAllStreamRenderTimers() {
  for (const timer of streamRenderTimers.values()) clearTimeout(timer)
  streamRenderTimers.clear()
  streamRenderLast.clear()
}

// ── Computed ──────────────────────────────────────────────────────────────────

const activeConv = computed(() =>
  conversations.value.find(c => c.id === activeConvId.value) ?? null
)
const activeMessages = computed(() => activeConv.value?.messages ?? [])

// Left-rail message navigation: one tick per user message, hover previews the
// text, click scrolls to it. Mirrors the per-paper AI chat (AiTab) rail.
const messageNav = computed(() =>
  activeMessages.value
    .filter(m => m.role === 'user')
    .map(m => ({ id: m.id, preview: m.content.trim() || '（空消息）' })),
)

function scrollToMessage(id: string) {
  const container = messagesEl.value
  if (!container) return
  const el = container.querySelector<HTMLElement>(`[data-msg-id="${CSS.escape(id)}"]`)
  if (!el) return
  const top = container.scrollTop + el.getBoundingClientRect().top - container.getBoundingClientRect().top - 12
  container.scrollTo({ top: Math.max(0, top), behavior: 'smooth' })
}

// The rail preview is teleported to <body> so no ancestor's `overflow: hidden`
// can clip it.
const navTip = ref<{ preview: string; x: number; y: number } | null>(null)
// Index of the hovered tick, driving the "wave": the hovered tick is longest and
// its neighbours elongate progressively less with distance.
const hoveredNavIndex = ref<number | null>(null)

const RAIL_BASE = 9
const RAIL_PEAK = 24
const RAIL_FALLOFF = 5

function railLineWidth(index: number): number {
  const h = hoveredNavIndex.value
  if (h === null) return RAIL_BASE
  const d = Math.abs(index - h)
  return Math.max(RAIL_BASE, RAIL_PEAK - d * RAIL_FALLOFF)
}

function onNavHover(index: number, preview: string, e: Event) {
  hoveredNavIndex.value = index
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  const y = Math.min(Math.max(r.top + r.height / 2, 56), window.innerHeight - 56)
  // Anchor to the rail's left edge + the widest possible line + a gap, so the
  // tooltip never overlaps the tick as it animates out to its peak width.
  navTip.value = { preview, x: r.left + RAIL_PEAK + 14, y }
}
function clearNavHover() {
  hoveredNavIndex.value = null
  navTip.value = null
}

const canSend = computed(() =>
  (input.value.trim().length > 0 || attachments.value.length > 0) &&
  !loading.value &&
  ai.isConfigured &&
  (knowledgeSource.value !== 'papers' || selectedPaperSlugs.value.length > 0)
)
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
    snippet: t('libraryChat.sourceSnippet'),
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
  const provider = ai.settings.providers.find(p => p.id === model.providerId)
  // "openai_compatible" is a generic adapter kind used for DeepSeek/Kimi/etc.
  // It should not be treated as the OpenAI brand.
  const kind = provider?.kind === 'openai_compatible' ? '' : provider?.kind
  const haystack = [
    kind,
    model.providerId,
    model.providerName,
    model.modelId,
    model.displayName,
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase()

  // Check model-specific brands first so generic adapter kinds don't win.
  if (haystack.includes('deepseek')) return modelIconMap.deepseek
  if (haystack.includes('kimi') || haystack.includes('moonshot')) return modelIconMap.kimi
  if (haystack.includes('claude') || haystack.includes('anthropic')) return modelIconMap.claude
  if (haystack.includes('gemma')) return modelIconMap.gemma
  if (haystack.includes('gemini') || haystack.includes('google')) return modelIconMap.gemini
  if (haystack.includes('qwen') || haystack.includes('通义') || haystack.includes('alibaba')) {
    return modelIconMap.qwen ?? modelIconMap.alibaba
  }
  if (haystack.includes('grok') || haystack.includes('xai')) return modelIconMap.grok ?? modelIconMap.xai
  if (haystack.includes('zhipu') || haystack.includes('智谱') || haystack.includes('glm')) return modelIconMap.zhipu
  if (haystack.includes('baidu') || haystack.includes('ernie')) return modelIconMap.baidu
  if (haystack.includes('doubao') || haystack.includes('bytedance')) return modelIconMap.bytedance
  if (haystack.includes('mistral') || haystack.includes('huggingface')) return modelIconMap.huggingface
  if (haystack.includes('openai') || haystack.includes('gpt')) return modelIconMap.openai
  // Ollama is a host, not a model brand — the provider name pollutes the
  // haystack, so match its mark only after every real model brand above.
  if (haystack.includes('ollama')) return modelIconMap['ollama-color']

  // Fall back to filename-based matching for less common providers.
  const keys = [
    model.providerId,
    model.providerName,
    model.modelId.split('/')[0],
    model.displayName.split(':')[0],
  ]
  for (const raw of keys) {
    const key = raw.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
    if (modelIconMap[key]) return modelIconMap[key]
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

function answerModelOption(answer: LibraryAnswerVariant) {
  return ai.findModel(answer.model ?? null)
}

function answerModelLogo(answer: LibraryAnswerVariant) {
  return modelLogo(answerModelOption(answer))
}

function answerModelName(answer: LibraryAnswerVariant) {
  const model = answerModelOption(answer)
  if (!model) return answer.modelLabel ?? answer.model?.modelId ?? '默认模型'
  return model.providerName ? `${model.providerName} · ${model.displayName}` : model.displayName
}

function modelFallbackInitial(answer: LibraryAnswerVariant) {
  return (answer.modelLabel ?? answer.model?.modelId ?? 'AI').trim().charAt(0).toUpperCase() || 'AI'
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
    reasoningContent: msg.reasoningContent,
    contextContent: msg.contextContent,
    contextPaperLabels: msg.contextPaperLabels,
    inputTokens: msg.inputTokens,
    outputTokens: msg.outputTokens,
    totalTokens: msg.totalTokens,
    costUsd: msg.costUsd,
    startedAt: msg.startedAt,
    endedAt: msg.endedAt,
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
      contextContent: msg.contextContent,
      inputTokens: msg.inputTokens,
      outputTokens: msg.outputTokens,
      totalTokens: msg.totalTokens,
      costUsd: msg.costUsd,
      startedAt: msg.startedAt,
      endedAt: msg.endedAt,
    }]
    msg.activeVariantId = msg.variants[0].id
  }
  return msg.variants
}

function answerContextSections(answer: LibraryAnswerVariant) {
  return answer.contextContent?.sections?.filter(s => s.content?.trim()) ?? []
}

// The assistant answer that replies to a given user turn. Each user message is
// immediately followed by its assistant message, whose active answer carries the
// context the backend emitted for that turn.
function answerForUserTurn(userMsg: LibraryUiMessage): LibraryAnswerVariant | null {
  const msgs = activeMessages.value
  const idx = msgs.findIndex(m => m.id === userMsg.id)
  if (idx < 0) return null
  const ans = msgs[idx + 1]
  if (!ans || ans.role !== 'assistant') return null
  return activeAnswer(ans)
}

// Titles of the papers attached to this user turn. Prefers the persisted labels
// (they survive a reload); falls back to the transient section list.
function turnPaperLabels(userMsg: LibraryUiMessage): string[] {
  const a = answerForUserTurn(userMsg)
  if (!a) return []
  if (a.contextPaperLabels?.length) return a.contextPaperLabels
  return answerContextSections(a).map(s => s.label)
}

// Full text of a paper label on this turn, for the expandable preview — present
// only while the transient content is still in memory (gone after a reload, where
// the badge still shows but the preview can't).
function turnPaperContent(userMsg: LibraryUiMessage, label: string): string {
  const a = answerForUserTurn(userMsg)
  return a?.contextContent?.sections?.find(s => s.label === label)?.content ?? ''
}

// Papers this user turn introduces for the FIRST time (vs. earlier turns). The
// model still receives every selected paper each turn — they live in the system
// prompt, so we never stop sending them — but the badge is only shown on the turn
// that first added a paper, like an attachment announced once, not on every reply.
function newlyAddedPapers(userMsg: LibraryUiMessage): string[] {
  const seen = new Set<string>()
  for (const m of activeMessages.value) {
    if (m.role !== 'user') continue
    const labels = turnPaperLabels(m)
    if (m.id === userMsg.id) {
      return labels.filter(l => !seen.has(l))
    }
    for (const l of labels) seen.add(l)
  }
  return []
}

function answerUsedPdf(answer: LibraryAnswerVariant) {
  return answer.sources?.some(s => s.source_type === 'pdf')
}

function toggleContextPanel(answerId: string) {
  expandedContextId.value = expandedContextId.value === answerId ? null : answerId
}

const usdToCnyRate = computed(() => {
  const r = Number(settingsStore.settings.usd_to_cny_rate)
  return Number.isFinite(r) && r > 0 ? r : 7.2
})

function formatTokenCount(value: number | undefined) {
  if (typeof value !== 'number' || !Number.isFinite(value)) return ''
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 1 : 2)}M`
  if (value >= 10_000) return `${(value / 1_000).toFixed(1)}k`
  return String(value)
}

function formatCostCny(costUsd: number | null | undefined) {
  if (typeof costUsd !== 'number' || !Number.isFinite(costUsd) || costUsd < 0) return ''
  const cny = costUsd * usdToCnyRate.value
  if (cny < 0.01) return '<0.01'
  return cny.toFixed(cny < 1 ? 3 : 2)
}

function hasUsage(answer: LibraryAnswerVariant) {
  return typeof answer.inputTokens === 'number' || typeof answer.outputTokens === 'number'
}

function answerSpeed(answer: LibraryAnswerVariant) {
  if (!answer.startedAt || typeof answer.outputTokens !== 'number') return ''
  const end = answer.endedAt ?? performance.now()
  const seconds = Math.max(0.2, (end - answer.startedAt) / 1000)
  const speed = Math.round(answer.outputTokens / seconds)
  return speed > 0 ? `~${speed} tok/s` : ''
}

function chatHistoryFromMessages(messages: LibraryUiMessage[]): ChatMessage[] {
  const history: ChatMessage[] = []
  for (const m of messages) {
    if (m.role === 'user') {
      // Keep attachments inside the user message so they travel with the
      // conversation history. Each PDF only needs to be uploaded once; later
      // turns can reference it via the prior messages.
      if (m.attachments?.length) {
        history.push({ role: 'user', content: buildUserContentParts(m.content, m.attachments) })
      } else {
        history.push({ role: 'user', content: m.content })
      }
    } else {
      const ans = activeAnswer(m)
      if (ans.streaming || ans.error || !ans.content.trim()) continue
      history.push({ role: 'assistant', content: ans.content })
    }
  }
  return history
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
  await emitTo('main', 'argus-open-paper', {
    slug: group.slug,
    title: group.paper_title,
  }).catch(() => {})
  await invoke('focus_main_window').catch(() => {})
  window.setTimeout(() => {
    invoke('focus_main_window').catch(() => {})
  }, 120)
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

function resetNewConversationContext() {
  input.value = ''
  paperPickerOpen.value = false
  paperPickerSearch.value = ''
  sourcePickerOpen.value = false
  modelPickerMsgId.value = null
  expandedSources.value = []
  expandedContextId.value = null
  editingMsgId.value = null
  editingText.value = ''
  nextTick(autoResize)
}

function newConversation() {
  const conv: LibraryConversation = {
    id: genId(),
    title: t('libraryChat.untitled'),
    messages: [],
    selectedPaperSlugs: [],
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  }
  conversations.value.unshift(conv)
  activeConvId.value = conv.id
  persistConversations(conversations.value)
}

function startNewConversation() {
  resetNewConversationContext()
  newConversation()
}

function selectConversation(id: string) { activeConvId.value = id }

function deleteConversation(id: string) {
  conversations.value = conversations.value.filter(c => c.id !== id)
  if (activeConvId.value === id) {
    if (conversations.value.length > 0) activeConvId.value = conversations.value[0].id
    else newConversation()
  }
  persistConversations(conversations.value)
}

function persistActive() {
  if (!activeConv.value) return
  activeConv.value.updatedAt = new Date().toISOString()
  const idx = conversations.value.findIndex(c => c.id === activeConvId.value)
  if (idx > 0) {
    const [conv] = conversations.value.splice(idx, 1)
    conversations.value.unshift(conv)
  }
  persistConversations(conversations.value)
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

function openFilePicker() {
  fileInputRef.value?.click()
}

function addAttachmentFromFile(file: File) {
  if (!file.type.startsWith('image/') && file.type !== 'application/pdf') return false
  const reader = new FileReader()
  reader.onload = () => {
    const dataUrl = reader.result as string
    const type: Attachment['type'] = file.type.startsWith('image/') ? 'image' : 'pdf'
    const name = file.name || (type === 'image' ? 'pasted-image.png' : 'pasted-file.pdf')
    attachments.value.push({ id: crypto.randomUUID(), type, name, dataUrl })
  }
  reader.readAsDataURL(file)
  return true
}

function onFileSelected(e: Event) {
  const target = e.target as HTMLInputElement
  const files = target.files
  if (!files) return
  for (const file of Array.from(files)) {
    addAttachmentFromFile(file)
  }
  target.value = ''
}

function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items
  if (!items) return
  let consumed = false
  for (const item of Array.from(items)) {
    const file = item.getAsFile()
    if (!file) continue
    if (addAttachmentFromFile(file)) consumed = true
  }
  if (consumed) e.preventDefault()
}

function removeAttachment(id: string) {
  attachments.value = attachments.value.filter(a => a.id !== id)
}

function previewAttachment(att: Attachment) {
  if (att.type === 'image') {
    previewImage.value = att.dataUrl
  } else {
    previewPdf.value = att.dataUrl
  }
}

function closePreview() {
  previewImage.value = null
  previewPdf.value = null
}

function buildUserContentParts(text: string, atts?: Attachment[]): ChatContentPart[] {
  const parts: ChatContentPart[] = [{ type: 'text', text }]
  for (const att of atts ?? []) {
    if (att.type === 'image') {
      parts.push({ type: 'image_url', image_url: { url: att.dataUrl } })
    } else {
      parts.push({ type: 'file', file: { filename: att.name, file_data: att.dataUrl } })
    }
  }
  return parts
}

async function runAssistantRequest(
  conv: LibraryConversation,
  assistantMsg: LibraryUiMessage,
  target: LibraryUiMessage | LibraryAnswerVariant,
  history: ChatMessage[],
  sel: ModelSelection | null,
) {
  const eventSafeId = target.id.replace(/[^A-Za-z0-9:_/-]/g, '-')
  const eventName = `library-chat-${eventSafeId}`
  const sourcesEventName = `${eventName}-sources`
  const contextEventName = `${eventName}-context`
  const usageEventName = `${eventName}-usage`
  const reasoningEventName = `${eventName}-reasoning`
  // Sources are collected per-request in a closure local (was a shared
  // module-level array that concurrent requests overwrote).
  let pendingSources: RetrievedChunk[] = []
  target.content = ''
  target.displayContent = ''
  target.error = false
  target.streaming = true
  target.sources = undefined
  target.reasoningContent = undefined
  target.contextContent = undefined
  target.contextPaperLabels = undefined
  target.inputTokens = undefined
  target.outputTokens = undefined
  target.totalTokens = undefined
  target.costUsd = undefined
  target.startedAt = performance.now()
  target.endedAt = undefined
  target.model = sel
  target.modelLabel = modelLabel(sel)
  assistantMsg.streaming = true
  loading.value = true
  // Backend cancellation id: generated per request, sent to `chat_with_library`,
  // and used by stopStreaming to invoke `cancel_ai_request`.
  const requestId = crypto.randomUUID()
  activeRequestIds.set(target.id, requestId)
  stoppedTargetIds.delete(target.id)
  scrollToBottom()

  // All listeners for this request are tracked under target.id so a later
  // request (or unmount / stop) can tear down exactly this request's listeners.
  detachListeners(target.id)
  const offs: UnlistenFn[] = []
  activeUnlisteners.set(target.id, offs)

  offs.push(await listen<RetrievedChunk[]>(sourcesEventName, (e) => {
    pendingSources = e.payload ?? []
  }))

  offs.push(await listen<LibrarySentContextPayload>(contextEventName, (e) => {
    const sections = e.payload?.sections?.filter(s => s.content?.trim()) ?? []
    target.contextContent = { mode: e.payload?.mode, sections }
    // Persist just the labels (survives the transient-content strip) so the badge
    // and its dedup keep working after the conversation is reloaded.
    target.contextPaperLabels = sections.map(s => s.label)
    persistActive()
  }))

  // Only collect reasoning when the user turned thinking mode on — some models
  // (e.g. DeepSeek) stream reasoning_content by default, and we don't want the
  // 思考过程 box to appear unless it was explicitly requested.
  if (useReasoning.value) {
    offs.push(await listen<{ delta?: string; done?: boolean }>(reasoningEventName, (e) => {
      if (e.payload.done) return
      if (stoppedTargetIds.has(target.id)) return
      const delta = e.payload.delta ?? ''
      if (!delta) return
      target.reasoningContent = (target.reasoningContent ?? '') + delta
      scrollToBottom()
    }))
  }

  offs.push(await listen<StreamUsagePayload>(usageEventName, (e) => {
    const usage = e.payload
    if (typeof usage.input_tokens === 'number') target.inputTokens = usage.input_tokens
    if (typeof usage.output_tokens === 'number') target.outputTokens = usage.output_tokens
    if (typeof usage.total_tokens === 'number') target.totalTokens = usage.total_tokens
    if (typeof usage.cost_usd === 'number' || usage.cost_usd === null) target.costUsd = usage.cost_usd
    persistActive()
  }))

  offs.push(await listen<{ delta?: string; done?: boolean }>(eventName, (e) => {
    if (e.payload.done) return
    if (stoppedTargetIds.has(target.id)) return
    const delta = e.payload.delta ?? ''
    if (!delta) return
    target.content += delta
    // Throttle the heavy markdown render instead of re-rendering every token.
    scheduleStreamRender(target)
  }))

  try {
    const requestPaperSlugs = knowledgeSource.value === 'papers'
      ? normalizeSelectedPaperSlugs(conv.selectedPaperSlugs)
      : []
    // DeepSeek only exposes two levels; the backend maps 'medium'->high, 'high'->max.
    const provider = ai.settings.providers.find(p => p.id === sel?.providerId)
    const isDeepseek = !!provider?.base_url.toLowerCase().includes('deepseek')
    const effortToSend = isDeepseek
      ? (reasoningLevel.value === 'high' ? 'high' : 'medium')
      : reasoningLevel.value
    const finalText = await invoke<string>('chat_with_library', {
      messages: history,
      providerId: sel?.providerId ?? null,
      modelId: sel?.modelId ?? null,
      eventName,
      sourcesEventName,
      knowledgeSource: knowledgeSource.value,
      selectedPaperSlugs: requestPaperSlugs,
      attachments: null,
      useReasoning: useReasoning.value,
      reasoningEffort: useReasoning.value ? effortToSend : null,
      requestId,
    })
    // If the user pressed stop, don't refill content the backend produced anyway.
    if (!stoppedTargetIds.has(target.id)) {
      if (!target.content && finalText) target.content = finalText
      if (pendingSources.length > 0) target.sources = [...pendingSources]
    }
    target.streaming = false
    target.endedAt = performance.now()
    assistantMsg.streaming = false
    flushStreamRender(target)
    persistActive()
    // Auto-generate title after the first exchange (fire-and-forget)
    if (conv.messages.filter((m: LibraryUiMessage) => m.role === 'user').length === 1) {
      generateAiTitle(conv)
    }
  } catch (e) {
    if (!stoppedTargetIds.has(target.id)) {
      target.content = String(e)
      target.error = true
    }
    target.streaming = false
    target.endedAt = performance.now()
    assistantMsg.streaming = false
    flushStreamRender(target)
  } finally {
    loading.value = false
    stoppedTargetIds.delete(target.id)
    activeRequestIds.delete(target.id)
    detachListeners(target.id)
    persistActive()
    scrollToBottom()
  }
}

// Off + drop all event listeners registered for a streaming target.
function detachListeners(targetId: string) {
  const offs = activeUnlisteners.get(targetId)
  if (offs) {
    for (const off of offs) off()
    activeUnlisteners.delete(targetId)
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

// Stop all in-flight streaming: tell the backend to cancel the HTTP request
// (via `cancel_ai_request`), detach front-end listeners, mark the streaming
// targets stopped (so the pending `finalText` won't refill their content), and
// reset UI state.
function stopStreaming() {
  // Tell the backend to truly cancel each in-flight request (closes the HTTP
  // stream so the provider stops generating / billing).
  for (const requestId of [...activeRequestIds.values()]) {
    invoke('cancel_ai_request', { requestId }).catch(() => {})
  }
  for (const id of [...activeUnlisteners.keys()]) {
    stoppedTargetIds.add(id)
    detachListeners(id)
  }
  const conv = activeConv.value
  if (conv) {
    for (const msg of conv.messages) {
      const targets: StreamTarget[] = [msg, ...(msg.variants ?? [])]
      for (const target of targets) {
        if (target.streaming) {
          target.streaming = false
          target.endedAt = performance.now()
          flushStreamRender(target)
        }
      }
    }
  }
  loading.value = false
  persistActive()
}

async function sendMessage() {
  const text = input.value.trim()
  if ((!text && !attachments.value.length) || loading.value) return

  if (!activeConvId.value) newConversation()
  const conv = activeConv.value!
  const sel = effectiveModel()
  const currentAttachments = attachments.value.length > 0 ? [...attachments.value] : undefined
  input.value = ''
  attachments.value = []
  nextTick(autoResize)

  if (conv.messages.filter(m => m.role === 'user').length === 0) {
    conv.title = deriveTitleFromMsg(text)
  }

  conv.messages.push({
    id: genId(), role: 'user', content: text, attachments: currentAttachments, createdAt: new Date().toISOString(),
  })
  conv.messages.push(createAssistantMessage(sel))
  // Use the reactive reference from the array so Vue tracks mutations during streaming
  const reactiveMsg = conv.messages[conv.messages.length - 1] as LibraryUiMessage
  const history = buildHistoryBeforeMessage(conv, reactiveMsg.id)
  await runAssistantRequest(conv, reactiveMsg, reactiveMsg, history, sel)
}

// Regenerate = REPLACE current answer in place (same model, no new variant)
async function regenerateAssistant(msg: LibraryUiMessage) {
  if (loading.value || msg.role !== 'assistant' || !activeConv.value) return
  const conv = activeConv.value
  const reactiveMsg = conv.messages.find(m => m.id === msg.id) as LibraryUiMessage | undefined
  if (!reactiveMsg) return
  const sel = effectiveModel()
  reactiveMsg.variants = []
  reactiveMsg.activeVariantId = undefined
  const history = buildHistoryBeforeMessage(conv, msg.id)
  await runAssistantRequest(conv, reactiveMsg, reactiveMsg, history, sel)
}

// @ model = generate NEW variant with selected model (for comparison)
async function regenerateWithModel(msg: LibraryUiMessage, modelSel: ModelOption) {
  if (loading.value || msg.role !== 'assistant' || !activeConv.value) return
  modelPickerMsgId.value = null
  const conv = activeConv.value
  const reactiveMsg = conv.messages.find(m => m.id === msg.id) as LibraryUiMessage | undefined
  if (!reactiveMsg) return
  const sel: ModelSelection = { providerId: modelSel.providerId, modelId: modelSel.modelId }
  const variants = ensureAnswerVariants(reactiveMsg)
  variants.push({
    id: `${msg.id}:v${variants.length}`,
    content: '',
    createdAt: new Date().toISOString(),
    streaming: true,
    model: sel,
    modelLabel: modelLabel(sel),
  })
  reactiveMsg.activeVariantId = variants[variants.length - 1].id
  // Get reactive reference from array so streaming triggers Vue updates
  const reactiveVariant = variants[variants.length - 1]
  const history = buildHistoryBeforeMessage(conv, msg.id)
  await runAssistantRequest(conv, reactiveMsg, reactiveVariant, history, sel)
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
  conv.messages.push(createAssistantMessage(sel))
  const reactiveMsg = conv.messages[conv.messages.length - 1] as LibraryUiMessage
  const history = buildHistoryBeforeMessage(conv, reactiveMsg.id)
  await runAssistantRequest(conv, reactiveMsg, reactiveMsg, history, sel)
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

// `isComposing` / keyCode 229 reliably detect an active IME composition,
// avoiding the race-prone Date.now() heuristic that could send half-typed text.
function isIMEActive(e: KeyboardEvent) { return e.isComposing || e.keyCode === 229 }

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey && !isIMEActive(e)) { e.preventDefault(); sendMessage() }
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
  const target = e.target as HTMLElement
  if (!target.closest('.lc-model-picker')) {
    modelMenuOpen.value = false
  }
  if (!target.closest('.msg-model-picker') && !target.closest('.msg-model-menu-teleport')) {
    modelPickerMsgId.value = null
  }
  if (!target.closest('.ks-picker')) {
    sourcePickerOpen.value = false
  }
  if (!target.closest('.reasoning-picker')) {
    reasoningOpen.value = false
  }
}

watch(selectedModel, (sel) => {
  if (ai.loaded) persistSelectedModel(sel)
})

watch(() => ai.chatModels.map(selectionKey).join('|'), () => {
  if (!ai.loaded) return
  if (!validSelection(selectedModel.value)) restoreLastModel()
})

function onCopyCode(e: Event) {
  navigator.clipboard.writeText((e.target as HTMLElement).textContent ?? '').catch(() => {})
}

// Switching conversations abandons any in-flight stream for the previous one:
// detach its listeners and clear pending throttle timers so scheduled renders
// don't fire against a conversation that's no longer visible.
watch(activeConvId, () => {
  for (const id of [...activeUnlisteners.keys()]) {
    stoppedTargetIds.add(id)
    detachListeners(id)
  }
  clearAllStreamRenderTimers()
  loading.value = false
})

async function refreshConversations() {
  const saved = await loadConversations()
  conversations.value = saved
  if (saved.length > 0) activeConvId.value = saved[0].id
  else newConversation()
}

let unlistenLibraryChanged: UnlistenFn | null = null

onMounted(async () => {
  await settingsStore.load()
  await refreshConversations()

  if (!ai.loaded) await ai.load()
  restoreLastModel()
  if (!ragStore.loaded) await ragStore.load()
  await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts(), loadSnippetStoreCounts()])
  document.addEventListener('mousedown', closeModelMenu)

  messagesEl.value?.addEventListener('copy-code', onCopyCode)

  // The library-chat window persists across library switches, so reload all
  // per-library data when the active library changes — otherwise it would show
  // (and, worse, save) the previous library's conversations against the new one.
  unlistenLibraryChanged = await listen('library-changed', async () => {
    for (const id of [...activeUnlisteners.keys()]) {
      stoppedTargetIds.add(id)
      detachListeners(id)
    }
    clearAllStreamRenderTimers()
    loading.value = false
    resetNewConversationContext()
    await ai.load()
    await ragStore.load()
    restoreLastModel()
    await Promise.all([ragStore.loadStoreInfo(), loadPaperCounts(), loadSnippetStoreCounts()])
    await refreshConversations()
  })

  // Window size is persisted by the Tauri window event handler.
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeModelMenu)
  window.removeEventListener('mousemove', onDividerMouseMove)
  window.removeEventListener('mouseup', onDividerMouseUp)
  messagesEl.value?.removeEventListener('copy-code', onCopyCode)
  for (const id of [...activeUnlisteners.keys()]) detachListeners(id)
  clearAllStreamRenderTimers()
  unlistenLibraryChanged?.()
})
</script>

<template>
  <div class="lc-root">

    <!-- ── Unified titlebar (full-width, drag region) ───────────────────────── -->
    <div class="lc-titlebar" :class="{ 'win-titlebar': isWindows }" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <template v-if="ai.loaded && ai.isConfigured">
        <div class="header-avatar" data-tauri-drag-region>
          <Icon icon="fluent:chat-24-regular" width="15" height="15" data-tauri-drag-region />
        </div>
        <div class="header-title-block" data-tauri-drag-region>
          <span class="header-conv-title" data-tauri-drag-region>{{ activeConv?.title || t('libraryChat.untitled') }}</span>
          <span class="header-subtitle" data-tauri-drag-region>{{ conversationSubtitle }}</span>
        </div>
        <div class="lc-titlebar-fill" data-tauri-drag-region />
        <div class="lc-titlebar-actions">
          <!-- RAG not configured -->
          <button v-if="knowledgeSource !== 'papers' && !ragStore.isConfigured" class="rag-badge inactive" title="点击配置 RAG" @click="emit('open-settings', 'rag')">
            <Icon icon="fluent:database-24-regular" width="11" height="11" />
            RAG
          </button>
          <template v-else-if="knowledgeSource === 'papers'">
            <div class="paper-context-counter" :title="selectedPapers.map(p => p.title).join('\n') || '尚未添加文献'">
              {{ selectedPapers.length }} 篇
            </div>
            <button class="rag-refresh-btn" title="添加文献" @click="openPaperPicker">
              <Icon icon="fluent:add-24-regular" width="15" height="15" />
            </button>
          </template>
          <template v-else-if="knowledgeSource === 'snippets'">
            <!-- Snippet RAG controls -->
            <span v-if="snippetSyncing" class="rag-sync-progress">{{ snippetSyncProgress.done }}/{{ snippetSyncProgress.total }}</span>
            <button class="rag-refresh-btn" :class="{ refreshing: snippetSyncing }" title="刷新素材库嵌入状态" :disabled="snippetSyncing" @click="loadSnippetStoreCounts">
              <Icon icon="fluent:arrow-sync-24-regular" width="15" height="15" />
            </button>
            <div class="rag-counter" title="素材库：已嵌入素材 / 总素材数">
              <Icon icon="fluent:database-24-regular" width="11" height="11" />
              <span class="rag-counter-text">{{ snippetEmbeddedCount }}/{{ snippetTotalCount }}</span>
            </div>
            <button
              class="rag-sync-btn"
              :class="{ 'all-done': snippetEmbeddedCount >= snippetTotalCount && snippetTotalCount > 0 }"
              :title="snippetEmbeddedCount < snippetTotalCount ? `嵌入 ${snippetTotalCount - snippetEmbeddedCount} 条未向量化的素材` : '所有素材已嵌入'"
              :disabled="snippetSyncing || (snippetEmbeddedCount >= snippetTotalCount && snippetTotalCount > 0)"
              @click="syncSnippets"
            >
              <Icon v-if="snippetEmbeddedCount < snippetTotalCount" icon="fluent:cloud-arrow-up-24-regular" width="11" height="11" />
              <Icon v-else icon="fluent:checkmark-24-regular" width="11" height="11" />
              {{ snippetSyncing ? '嵌入中…' : snippetEmbeddedCount < snippetTotalCount ? `嵌入 ${snippetTotalCount - snippetEmbeddedCount} 条` : '已全部嵌入' }}
            </button>
          </template>
          <!-- Paper RAG controls -->
          <template v-else-if="knowledgeSource === 'paper-rag'">
            <span v-if="syncingMissing" class="rag-sync-progress">{{ syncProgress.done }}/{{ syncProgress.total }}</span>
            <button class="rag-refresh-btn" :class="{ refreshing: refreshingCounts || syncingMissing }" title="刷新嵌入状态" :disabled="refreshingCounts || syncingMissing" @click="refreshCounts">
              <Icon icon="fluent:arrow-sync-24-regular" width="15" height="15" />
            </button>
            <div class="rag-counter" title="向量库：已嵌入论文 / 总论文数">
              <Icon icon="fluent:database-24-regular" width="11" height="11" />
              <span class="rag-counter-text">{{ vectorizedCount }}/{{ allPapers.length }}</span>
            </div>
            <template v-if="syncingMissing">
              <button class="rag-sync-cancel" @click="syncCancelRequested = true" title="取消同步"><Icon icon="fluent:dismiss-24-regular" width="11" height="11" /></button>
            </template>
            <button v-else class="rag-sync-btn" :class="{ 'all-done': unvectorizedPapers.length === 0 }" :title="unvectorizedPapers.length > 0 ? `嵌入 ${unvectorizedPapers.length} 篇未向量化的论文` : '所有论文已嵌入'" :disabled="unvectorizedPapers.length === 0" @click="syncMissing">
              <Icon v-if="unvectorizedPapers.length > 0" icon="fluent:cloud-arrow-up-24-regular" width="11" height="11" />
              <Icon v-else icon="fluent:checkmark-24-regular" width="11" height="11" />
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
              <Icon class="chevron" :class="{ open: modelMenuOpen }" icon="fluent:chevron-down-24-regular" width="12" height="12" />
            </button>
            <div v-if="modelMenuOpen" class="lc-model-menu">
              <div v-for="group in ai.groupedModels" :key="group.id" class="lc-model-group">
                <div class="lc-model-group-name">{{ group.name }}</div>
                <button
                  v-for="model in group.models"
                  :key="selectionKey(model)"
                  class="lc-model-row"
                  :class="{ active: selectionKey(model) === selectionKey(effectiveModel()) }"
                  @mousedown.prevent.stop="selectModel(model)"
                  @click.stop="selectModel(model)"
                >
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
      <WindowControls />
    </div>

    <!-- ── No AI provider ────────────────────────────────────────────────────── -->
    <div v-if="ai.loaded && !ai.isConfigured" class="center-hint">
      <div class="hint-icon">
        <Icon icon="fluent:info-24-regular" width="24" height="24" />
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
          <button class="new-chat-btn" :title="t('libraryChat.newChat')" @click="startNewConversation">
            <Icon icon="fluent:add-24-regular" width="14" height="14" />
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
              <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
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
              <Icon icon="fluent:panel-left-24-regular" width="15" height="15" />
            </button>
            <div class="header-avatar">
              <Icon icon="fluent:chat-24-regular" width="16" height="16" />
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
              <Icon icon="fluent:database-24-regular" width="11" height="11" />
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
                <Icon icon="fluent:arrow-sync-24-regular" width="15" height="15" />
              </button>

              <div class="rag-counter" title="向量库：已嵌入论文 / 总论文数">
                <Icon icon="fluent:database-24-regular" width="11" height="11" />
                <span class="rag-counter-text">{{ vectorizedCount }}/{{ allPapers.length }}</span>
              </div>

              <!-- Syncing: cancel button -->
              <template v-if="syncingMissing">
                <button class="rag-sync-cancel" @click="syncCancelRequested = true" title="取消同步">
                  <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
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
                <Icon v-if="unvectorizedPapers.length > 0" icon="fluent:cloud-arrow-up-24-regular" width="11" height="11" />
                <Icon v-else icon="fluent:checkmark-24-regular" width="11" height="11" />
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
                <Icon class="chevron" :class="{ open: modelMenuOpen }" icon="fluent:chevron-down-24-regular" width="12" height="12" />
              </button>

              <div v-if="modelMenuOpen" class="lc-model-menu">
                <div v-for="group in ai.groupedModels" :key="group.id" class="lc-model-group">
                  <div class="lc-model-group-name">{{ group.name }}</div>
                  <button
                    v-for="model in group.models"
                    :key="selectionKey(model)"
                    class="lc-model-row"
                    :class="{ active: selectionKey(model) === selectionKey(effectiveModel()) }"
                    @mousedown.prevent.stop="selectModel(model)"
                    @click.stop="selectModel(model)"
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
            <Icon icon="fluent:info-24-regular" width="13" height="13" />
          </div>
          <span class="rag-hint-text">{{ t('libraryChat.ragHint') }}</span>
          <button class="rag-hint-action" @click="emit('open-settings', 'rag')">{{ t('libraryChat.ragHintAction') }}</button>
        </div>

        <!-- Messages -->
        <div class="messages-wrap">
        <nav
          v-if="messageNav.length > 1"
          class="rail-nav"
          aria-label="消息导航"
          @mouseleave="clearNavHover"
        >
          <button
            v-for="(item, index) in messageNav"
            :key="item.id"
            type="button"
            class="rail-tick"
            @click="scrollToMessage(item.id)"
            @mouseenter="onNavHover(index, item.preview, $event)"
            @focus="onNavHover(index, item.preview, $event)"
            @blur="clearNavHover"
          >
            <span class="rail-line" :class="{ active: index === hoveredNavIndex }" :style="{ width: `${railLineWidth(index)}px` }" />
          </button>
        </nav>
        <Teleport to="body">
          <div v-if="navTip" class="rail-tooltip-float" :style="{ left: `${navTip.x}px`, top: `${navTip.y}px` }">
            {{ navTip.preview }}
          </div>
        </Teleport>
        <div ref="messagesEl" class="messages" @click="onMsgContainerClick">

          <!-- Empty state -->
          <div v-if="activeMessages.length === 0" class="empty-chat">
            <div class="empty-panel">
              <div class="empty-icon">
                <Icon icon="fluent:chat-24-regular" width="28" height="28" />
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
            <div v-if="msg.role === 'user'" class="msg-row user" :data-msg-id="msg.id">
              <div v-if="editingMsgId === msg.id" class="user-edit-card">
                <textarea
                  :id="`edit-${msg.id}`"
                  v-model="editingText"
                  class="user-edit-input"
                  rows="3"
                  @keydown.escape.prevent="cancelEdit"
                  @keydown.enter.exact.prevent="!isIMEActive($event) && submitUserEdit(msg)"
                />
                <div class="user-edit-actions">
                  <button class="edit-cancel" @click="cancelEdit">取消</button>
                  <button class="edit-submit" :disabled="!editingText.trim() || loading" @click="submitUserEdit(msg)">发送</button>
                </div>
              </div>
              <template v-else>
                <div class="user-message-stack">
                  <!-- Papers first attached on this turn (deduped against earlier turns) -->
                  <div v-if="newlyAddedPapers(msg).length" class="context-banner user-context-banner">
                    <button
                      class="ctx-pills"
                      :title="expandedContextId === msg.id ? '收起' : '查看发送给 AI 的文献'"
                      @click="toggleContextPanel(msg.id)"
                    >
                      <span
                        v-for="(label, ci) in newlyAddedPapers(msg)"
                        :key="`${msg.id}-ctx-${ci}`"
                        class="ctx-pill ctx-paper"
                        :title="label"
                      >{{ label }}</span>
                      <Icon class="ctx-chevron" :class="{ open: expandedContextId === msg.id }" icon="fluent:chevron-down-24-regular" width="11" height="11" />
                    </button>
                    <div v-if="expandedContextId === msg.id" class="ctx-preview">
                      <div
                        v-for="(label, ci) in newlyAddedPapers(msg)"
                        :key="`${msg.id}-ctx-preview-${ci}`"
                        class="ctx-section"
                      >
                        <div class="ctx-section-label">{{ label }}</div>
                        <pre v-if="turnPaperContent(msg, label)" class="ctx-preview-text">{{ turnPaperContent(msg, label) }}</pre>
                        <div v-else class="ctx-preview-text ctx-preview-empty">全文预览不可用（重新打开对话后不再保留）。</div>
                      </div>
                    </div>
                  </div>
                  <div v-if="msg.attachments && msg.attachments.length" class="user-attachments">
                    <button
                      v-for="att in msg.attachments"
                      :key="att.id"
                      class="user-attachment"
                      :class="{ pdf: att.type === 'pdf' }"
                      :title="att.name"
                      @click="previewAttachment(att)"
                    >
                      <img v-if="att.type === 'image'" :src="att.dataUrl" class="user-attachment-thumb" alt="" />
                      <Icon v-else icon="fluent:document-24-regular" width="14" height="14" />
                      <span class="user-attachment-name">{{ att.name }}</span>
                    </button>
                  </div>
                  <div class="user-bubble">{{ msg.content }}</div>
                  <div class="message-actions user-actions">
                    <button :title="copiedMsgIds.has(msg.id) ? '已复制' : '复制'" @click="copyMessage(msg)">
                      <Icon icon="fluent:copy-24-regular" width="13" height="13" />
                    </button>
                    <button title="编辑并重发" :disabled="loading" @click="startEditUser(msg)">
                      <Icon icon="fluent:edit-24-regular" width="13" height="13" />
                    </button>
                  </div>
                </div>
              </template>
            </div>

            <!-- Assistant -->
            <div v-else class="msg-row assistant">
              <div class="assistant-wrap">
                <div class="assistant-avatar" :title="answerModelName(activeAnswer(msg))">
                  <img v-if="answerModelLogo(activeAnswer(msg))" :src="answerModelLogo(activeAnswer(msg))" alt="" />
                  <span v-else>{{ modelFallbackInitial(activeAnswer(msg)) }}</span>
                </div>
                <div class="assistant-content">
                  <!-- Thinking / reasoning content (collapsible) -->
                  <div v-if="activeAnswer(msg).reasoningContent" class="reasoning-section">
                    <button
                      class="reasoning-summary"
                      @click="toggleReasoning(activeAnswer(msg).id)"
                    >
                      <Icon
                        width="11" height="11"
                        class="reasoning-chevron"
                        :class="{ collapsed: isReasoningCollapsed(activeAnswer(msg).id) }"
                        icon="fluent:chevron-down-24-regular"
                      />
                      思考过程
                      <span v-if="activeAnswer(msg).streaming && !activeAnswer(msg).content" class="reasoning-live-dot" />
                      <span class="reasoning-count">{{ reasoningStats(activeAnswer(msg).reasoningContent || '') }}</span>
                    </button>
                    <pre
                      v-show="!isReasoningCollapsed(activeAnswer(msg).id)"
                      class="reasoning-body"
                    >{{ activeAnswer(msg).reasoningContent }}</pre>
                  </div>

                  <div
                    class="assistant-bubble markdown-body"
                    :class="{ streaming: activeAnswer(msg).streaming, error: activeAnswer(msg).error }"
                  >
                    <!-- Streaming: render the throttled displayContent copy, not the
                         raw content, so long answers don't re-render on every token.
                         Before any content arrives we show just a blinking cursor —
                         no "思考中" placeholder (the 思考过程 box already covers thinking). -->
                    <template v-if="activeAnswer(msg).streaming">
                      <div v-if="activeAnswer(msg).content" v-html="renderMarkdown(activeAnswer(msg).displayContent ?? activeAnswer(msg).content)" />
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

                  <!-- Action buttons -->
                  <div v-if="!activeAnswer(msg).streaming || hasUsage(activeAnswer(msg))" class="assistant-action-row">
                    <div v-if="!activeAnswer(msg).streaming" class="message-actions assistant-actions">
                      <button :title="copiedMsgIds.has(msg.id) ? '已复制' : '复制'" @click="copyMessage(msg)">
                        <Icon icon="fluent:copy-24-regular" width="13" height="13" />
                      </button>
                      <button title="重新生成" :disabled="loading" @click="regenerateAssistant(msg)">
                        <Icon icon="fluent:arrow-sync-24-regular" width="13" height="13" />
                      </button>
                      <!-- @ button: pick another model and add as a variant -->
                      <div class="msg-model-picker" @click.stop>
                        <button
                          class="at-btn"
                          title="用其他模型回答"
                          :disabled="loading"
                          :class="{ active: modelPickerMsgId === msg.id }"
                          @click.stop="openModelPicker(msg.id, $event)"
                        >
                          <Icon icon="fluent:mention-24-regular" width="13" height="13" />
                        </button>
                      </div>
                    </div>
                    <div class="assistant-usage">
                      <span class="assistant-model-meta" :title="answerModelName(activeAnswer(msg))">
                        <span class="assistant-model-meta-icon">
                          <img v-if="answerModelLogo(activeAnswer(msg))" :src="answerModelLogo(activeAnswer(msg))" alt="" />
                          <span v-else>{{ modelFallbackInitial(activeAnswer(msg)) }}</span>
                        </span>
                        <span class="assistant-model-meta-name">{{ answerModelName(activeAnswer(msg)) }}</span>
                        <span
                          v-if="answerUsedPdf(activeAnswer(msg))"
                          class="pdf-badge"
                          title="已将选中文献的 PDF 直接发送给模型"
                        >PDF</span>
                      </span>
                      <span v-if="typeof activeAnswer(msg).inputTokens === 'number'" title="上下文输入 tokens">↑{{ formatTokenCount(activeAnswer(msg).inputTokens) }}</span>
                      <span v-if="typeof activeAnswer(msg).outputTokens === 'number'" title="本次输出 tokens">↓{{ formatTokenCount(activeAnswer(msg).outputTokens) }}</span>
                      <span v-if="answerSpeed(activeAnswer(msg))" class="msg-speed">{{ answerSpeed(activeAnswer(msg)) }}</span>
                      <span v-if="activeAnswer(msg).costUsd != null && formatCostCny(activeAnswer(msg).costUsd)" class="usage-cost" :title="`约 ¥${formatCostCny(activeAnswer(msg).costUsd)} / $${activeAnswer(msg).costUsd!.toFixed(6)}`">¥{{ formatCostCny(activeAnswer(msg).costUsd) }}</span>
                    </div>
                  </div>

                  <!-- Divider + meta row -->
                  <div
                    v-if="answerVariants(msg).length > 1 || (!activeAnswer(msg).streaming && answerSources(msg).length > 0)"
                    class="meta-row"
                  >
                    <div class="meta-divider" />
                    <div class="meta-content">
                      <!-- Sources toggle on the LEFT -->
                      <button
                        v-if="!activeAnswer(msg).streaming && answerSources(msg).length > 0"
                        class="sources-toggle"
                        @click="toggleSources(msg.id)"
                      >
                        <Icon icon="fluent:database-24-regular" width="11" height="11" />
                        <span>{{ isSourcesExpanded(msg.id) ? t('libraryChat.hideSources') : t('libraryChat.sources', { n: groupedSources(answerSources(msg)).length }) }}</span>
                        <Icon class="chevron" :class="{ open: isSourcesExpanded(msg.id) }" icon="fluent:chevron-down-24-regular" width="10" height="10" />
                      </button>
                      <!-- Model variant tabs to the RIGHT of sources -->
                      <div v-if="answerVariants(msg).length > 1" class="answer-tabs">
                        <button
                          v-for="(variant, index) in answerVariants(msg)"
                          :key="variant.id"
                          class="answer-tab"
                          :class="{ active: variant.id === msg.activeVariantId }"
                          :title="variant.modelLabel || `回答 ${index + 1}`"
                          @click="msg.activeVariantId = variant.id"
                        >
                          <img
                            v-if="variant.model && modelLogo(ai.findModel(variant.model))"
                            :src="modelLogo(ai.findModel(variant.model))"
                            alt=""
                          />
                          <span v-else class="tab-icon-fallback">{{ (variant.modelLabel || `${index + 1}`).charAt(0) }}</span>
                        </button>
                      </div>
                    </div>
                  </div>

                  <!-- Sources list (expanded) -->
                  <div v-if="isSourcesExpanded(msg.id) && answerSources(msg).length > 0" class="sources-list">
                    <div v-for="group in groupedSources(answerSources(msg))" :key="group.paper_id" class="source-group">
                      <button class="source-paper-name" @click="openSourcePaper(group)">
                        <Icon icon="fluent:document-24-regular" width="11" height="11" />
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

          </template>
        </div>
        </div>

        <!-- Input area -->
        <div class="input-area">
          <div class="composer">
            <div v-if="attachments.length" class="attachment-row">
              <div
                v-for="att in attachments"
                :key="att.id"
                class="attachment-chip"
                :class="{ pdf: att.type === 'pdf' }"
                :title="att.name"
              >
                <img v-if="att.type === 'image'" :src="att.dataUrl" class="attachment-thumb" alt="" />
                <Icon v-else icon="fluent:document-24-regular" width="14" height="14" />
                <span class="attachment-name">{{ att.name }}</span>
                <button class="attachment-remove" title="移除" @click="removeAttachment(att.id)">
                  <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
                </button>
              </div>
            </div>
            <textarea
              ref="textareaEl"
              v-model="input"
              class="chat-input"
              :placeholder="t('libraryChat.inputPlaceholder')"
              rows="1"
              :disabled="loading"
              @keydown="handleKeydown"
              @paste="onPaste"
            />
            <input
              ref="fileInputRef"
              type="file"
              accept="image/*,.pdf"
              multiple
              style="display: none"
              @change="onFileSelected"
            />
            <div class="composer-footer">
              <div class="footer-left">
                <button class="toolbar-btn" title="新建对话" @click="startNewConversation">
                  <Icon icon="fluent:compose-24-regular" width="15" height="15" />
                </button>
                <button
                  class="attach-btn"
                  title="添加图片或 PDF 附件"
                  :disabled="loading"
                  @click="openFilePicker"
                >
                  <Icon icon="fluent:attach-24-regular" width="14" height="14" />
                </button>
                <!-- Reasoning / thinking mode picker -->
                <div class="reasoning-picker" @click.stop>
                  <button
                    class="toolbar-btn"
                    :class="{ 'toolbar-btn-active': useReasoning }"
                    title="思考模式"
                    @click="reasoningOpen = !reasoningOpen"
                  >
                    <Icon icon="fluent:brain-circuit-24-regular" width="15" height="15" />
                    <span v-if="useReasoning" class="reasoning-badge">
                      {{ isDeepSeekSelected
                          ? (reasoningLevel === 'high' ? 'max' : 'high')
                          : (reasoningLevel === 'low' ? '低' : reasoningLevel === 'medium' ? '中' : '高') }}
                    </span>
                  </button>
                  <Transition name="reasoning-drop">
                    <div v-if="reasoningOpen" class="reasoning-popover">
                      <div class="reasoning-row">
                        <span class="reasoning-label">思考模式</span>
                        <button
                          class="reasoning-toggle"
                          :class="{ on: useReasoning }"
                          @click="useReasoning = !useReasoning"
                        >
                          <span class="toggle-knob" />
                        </button>
                      </div>
                      <div v-if="useReasoning" class="reasoning-levels">
                        <template v-if="isDeepSeekSelected">
                          <button
                            v-for="lv in (['high', 'max'] as const)"
                            :key="lv"
                            class="level-btn"
                            :class="{ active: lv === 'high' ? reasoningLevel === 'medium' : reasoningLevel === 'high' }"
                            @click="reasoningLevel = lv === 'max' ? 'high' : 'medium'"
                          >{{ lv }}</button>
                        </template>
                        <template v-else>
                          <button
                            v-for="lv in (['low', 'medium', 'high'] as const)"
                            :key="lv"
                            class="level-btn"
                            :class="{ active: reasoningLevel === lv }"
                            @click="reasoningLevel = lv"
                          >{{ lv === 'low' ? '低' : lv === 'medium' ? '中' : '高' }}</button>
                        </template>
                      </div>
                    </div>
                  </Transition>
                </div>
                <!-- Knowledge source picker -->
                <div class="ks-picker" @click.stop>
                  <button
                    class="ks-trigger"
                    :class="{
                      on: knowledgeSource === 'paper-rag' ? ragStore.isConfigured : true,
                      active: sourcePickerOpen,
                    }"
                    @click="sourcePickerOpen = !sourcePickerOpen"
                  >
                    <span class="ks-dot" />
                    {{ knowledgeSourceLabel }}
                    <Icon class="ks-chevron" :class="{ open: sourcePickerOpen }" icon="fluent:chevron-down-24-regular" width="10" height="10" />
                  </button>
                  <div v-if="sourcePickerOpen" class="ks-menu">
                    <button
                      class="ks-option"
                      :class="{ selected: knowledgeSource === 'paper-rag' }"
                      @click="setKnowledgeSource('paper-rag')"
                    >
                      <Icon icon="fluent:book-24-regular" width="12" height="12" />
                      <span class="ks-option-text">
                        文献库RAG
                        <span v-if="!ragStore.isConfigured" class="ks-option-hint">（RAG 未配置）</span>
                      </span>
                      <Icon v-if="knowledgeSource === 'paper-rag'" class="ks-check" icon="fluent:checkmark-24-regular" width="11" height="11" />
                    </button>
                    <button
                      class="ks-option"
                      :class="{ selected: knowledgeSource === 'papers' }"
                      @click="setKnowledgeSource('papers')"
                    >
                      <Icon icon="fluent:book-24-regular" width="12" height="12" />
                      <span class="ks-option-text">文献库</span>
                      <Icon v-if="knowledgeSource === 'papers'" class="ks-check" icon="fluent:checkmark-24-regular" width="11" height="11" />
                    </button>
                    <button
                      class="ks-option"
                      :class="{ selected: knowledgeSource === 'snippets' }"
                      @click="setKnowledgeSource('snippets')"
                    >
                      <Icon icon="fluent:document-text-24-regular" width="12" height="12" />
                      <span class="ks-option-text">素材库</span>
                      <Icon v-if="knowledgeSource === 'snippets'" class="ks-check" icon="fluent:checkmark-24-regular" width="11" height="11" />
                    </button>
                  </div>
                </div>
                <button
                  v-if="knowledgeSource === 'papers'"
                  class="add-paper-context-btn"
                  :class="{ 'has-count': selectedPapers.length > 0 }"
                  :title="selectedPapers.length > 0 ? `已选 ${selectedPapers.length} 篇文献` : '添加文献'"
                  @click="openPaperPicker"
                >
                  <Icon icon="fluent:add-24-regular" width="13" height="13" />
                  <span v-if="selectedPapers.length > 0" class="paper-count">{{ selectedPapers.length }}</span>
                </button>
              </div>
              <div class="footer-right">
                <span class="enter-hint">{{ t('libraryChat.enterHint') }}</span>
                <button v-if="loading" class="send-btn stop-btn" title="停止生成" @click="stopStreaming">
                  <Icon icon="fluent:stop-24-filled" width="13" height="13" />
                  停止
                </button>
                <button v-else class="send-btn" :disabled="!canSend" @click="sendMessage">
                  <Icon icon="fluent:send-24-regular" width="13" height="13" />
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

  <!-- Model picker teleported to body to escape overflow clipping -->
  <Teleport to="body">
    <div
      v-if="modelPickerMsgId && modelPickerMsg"
      class="msg-model-menu-teleport"
      :style="{ top: modelPickerPos.top + 'px', left: modelPickerPos.left + 'px' }"
      @click.stop
    >
      <div v-for="group in ai.groupedModels" :key="group.id" class="msg-model-group">
        <div class="msg-model-group-name">{{ group.name }}</div>
        <button
          v-for="model in group.models"
          :key="selectionKey(model)"
          class="msg-model-row"
          @click="regenerateWithModel(modelPickerMsg!, model)"
        >{{ model.displayName }}</button>
      </div>
    </div>
  </Teleport>

  <Teleport to="body">
    <div v-if="paperPickerOpen" class="paper-picker-overlay" @click.self="paperPickerOpen = false">
      <div class="paper-picker-dialog">
        <div class="paper-picker-header">
          <span class="paper-picker-title">添加文献</span>
          <button class="paper-picker-close" @click="paperPickerOpen = false">
            <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
          </button>
        </div>
        <input v-model="paperPickerSearch" class="paper-picker-search" placeholder="搜索标题、作者、年份..." autofocus />
        <div class="paper-picker-list">
          <div v-if="pickerPapers.length === 0" class="paper-picker-empty">暂无匹配文献</div>
          <button
            v-for="paper in pickerPapers"
            :key="paper.slug"
            class="paper-picker-item"
            :class="{ selected: selectedPaperSlugs.includes(paper.slug) }"
            @click="toggleSelectedPaper(paper)"
          >
            <span class="paper-picker-item-title">{{ paper.title }}</span>
            <span class="paper-picker-item-meta">
              {{ paper.authors.slice(0, 2).join(', ') }}{{ paper.authors.length > 2 ? ' 等' : '' }}
              <template v-if="paper.year"> · {{ paper.year }}</template>
            </span>
            <span v-if="selectedPaperSlugs.includes(paper.slug)" class="paper-picker-badge">点击移除</span>
          </button>
        </div>
      </div>
    </div>
  </Teleport>

  <!-- Attachment preview lightbox -->
  <Teleport to="body">
    <div v-if="previewImage" class="attachment-lightbox" @click.self="closePreview">
      <img :src="previewImage" class="lightbox-image" alt="" />
      <button class="lightbox-close" @click="closePreview">
        <Icon icon="fluent:dismiss-24-regular" width="18" height="18" />
      </button>
    </div>
    <div v-if="previewPdf" class="attachment-lightbox pdf-lightbox" @click.self="closePreview">
      <iframe :src="previewPdf" class="lightbox-pdf" frameborder="0"></iframe>
      <button class="lightbox-close" @click="closePreview">
        <Icon icon="fluent:dismiss-24-regular" width="18" height="18" />
      </button>
    </div>
  </Teleport>
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
/* Windows: no traffic lights, custom controls sit flush to the right edge. */
.lc-titlebar.win-titlebar { padding-right: 0; padding-left: 12px; }
.lc-titlebar.win-titlebar .tl-space { width: 0; }
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

.messages-wrap {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

/* Left navigation rail: one tick per user message (Codex-style). */
.rail-nav {
  position: absolute;
  left: 2px;
  top: 0;
  bottom: 0;
  width: 28px;
  z-index: 6;
  display: flex;
  flex-direction: column;
  justify-content: center;
  align-items: flex-start;
  gap: 7px;
  padding: 16px 0;
  overflow: hidden;
  pointer-events: none;   /* only the ticks are interactive */
}
.rail-tick {
  pointer-events: auto;
  position: relative;
  display: flex;
  align-items: center;
  height: 8px;
  padding: 0;
  border: none;
  background: none;
  cursor: pointer;
}
.rail-line {
  display: block;
  height: 2px;
  border-radius: 2px;
  background: var(--border-default);
  /* width is set inline (distance-based magnification); animate it. */
  transition: width .18s cubic-bezier(.34, 1.56, .64, 1), background .16s ease;
}
.rail-tick:hover .rail-line,
.rail-line.active {
  background: var(--accent);
}
/* Teleported to <body> so no ancestor overflow clips it. */
.rail-tooltip-float {
  position: fixed;
  transform: translateY(-50%);
  max-width: 260px;
  display: -webkit-box;
  -webkit-line-clamp: 5;
  -webkit-box-orient: vertical;
  overflow: hidden;
  padding: 8px 11px;
  border-radius: 8px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  box-shadow: var(--shadow-lg);
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.5;
  white-space: pre-wrap;
  text-align: left;
  pointer-events: none;
  z-index: 9999;
}

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
  align-items: flex-end;
}

.user-message-stack {
  max-width: min(76%, 680px);
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 5px;
  min-width: 0;
}

.user-bubble {
  max-width: 100%;
  padding: 11px 15px;
  background: color-mix(in srgb, var(--accent) 92%, #ffffff);
  color: #fff;
  border-radius: 16px;
  border-bottom-right-radius: 4px;
  font-size: 16px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  box-shadow: 0 6px 18px color-mix(in srgb, var(--accent) 18%, transparent);
  user-select: text;
  -webkit-user-select: text;
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
  overflow: hidden;
  font-size: 11px;
  font-weight: 700;
}

.assistant-avatar img {
  width: 22px;
  height: 22px;
  object-fit: contain;
}

.assistant-avatar span {
  line-height: 1;
}

.assistant-content {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

/* ── Thinking / reasoning box (思考过程) ───────────────────────────────────── */
.reasoning-section {
  margin-bottom: 6px;
  border: 1px solid var(--border-subtle);
  border-radius: 10px;
  background: color-mix(in srgb, var(--accent) 4%, transparent);
  overflow: hidden;
}
.reasoning-summary {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 12px;
  border: none;
  background: transparent;
  font-size: 12px;
  font-weight: 650;
  color: var(--text-secondary);
  cursor: pointer;
}
.reasoning-summary:hover { color: var(--text-primary); }
.reasoning-chevron {
  flex-shrink: 0;
  color: var(--text-tertiary);
  transform: rotate(180deg);
  transition: transform 0.15s ease;
}
.reasoning-chevron.collapsed { transform: rotate(0deg); }
.reasoning-live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
  animation: reasoning-pulse 1.15s infinite;
}
@keyframes reasoning-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}
.reasoning-count {
  margin-left: auto;
  font-size: 11px;
  font-weight: 500;
  color: var(--text-tertiary);
  white-space: nowrap;
}
/* The body is capped so it shows a preview and the rest scrolls — "显示一部分，
   剩下的折叠起来" — and the whole box collapses from the header. */
.reasoning-body {
  margin: 0;
  padding: 4px 13px 12px;
  max-height: 220px;
  overflow-y: auto;
  border-top: 1px solid var(--border-subtle);
  font-family: var(--font-sans);
  font-size: 12px;
  line-height: 1.62;
  color: var(--text-secondary);
  white-space: pre-wrap;
  word-break: break-word;
}

.assistant-bubble {
  padding: 2px 0;
  background: transparent;
  border: none;
  border-radius: 0;
  font-size: 16px;
  line-height: 1.72;
  color: var(--text-primary);
  word-break: break-word;
  overflow-x: auto;
  user-select: text;
  -webkit-user-select: text;
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
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-tertiary);
}

.message-actions button:hover:not(:disabled) {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}

.message-actions button:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}

.user-actions {
  justify-content: flex-end;
}

.assistant-actions {
  margin-top: 2px;
}

.assistant-action-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 29px;
  margin-top: 2px;
}

.assistant-usage {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  justify-content: flex-end;
  gap: 7px;
  min-width: 0;
  color: var(--text-tertiary);
  font-size: 10.5px;
  line-height: 1;
  white-space: nowrap;
}

.assistant-model-meta {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  max-width: min(320px, 42vw);
  color: var(--text-tertiary);
  font-size: 12px;
  font-weight: 500;
}

.assistant-model-meta-icon {
  width: 14px;
  height: 14px;
  border-radius: 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  overflow: hidden;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 7%, transparent);
  font-size: 8px;
  font-weight: 700;
}

.assistant-model-meta-icon img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}

.assistant-model-meta-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.assistant-usage .msg-speed {
  color: color-mix(in srgb, var(--accent) 74%, var(--text-tertiary));
}

.assistant-usage .usage-cost {
  color: var(--text-secondary);
  font-weight: 500;
  margin-left: 2px;
}
.pdf-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 15px;
  padding: 0 5px;
  border-radius: var(--radius-pill);
  background: #fff0f0;
  border: 1px solid #f0c0c0;
  color: #8b1e1e;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
}

/* ── Sent context banner ─────────────────────────────────────────────────── */
.context-banner {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  width: 100%;
  margin-bottom: 4px;
}

/* On the user turn the banner sits above the right-aligned bubble, so its pills
   and expanded preview hug the right edge. */
.user-context-banner {
  align-items: flex-end;
  margin-bottom: 6px;
}
.user-context-banner .ctx-preview {
  align-self: stretch;
}

.ctx-pills {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
  padding: 3px 7px 3px 6px;
  border-radius: 8px;
  border: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-secondary) 70%, transparent);
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
  transition: background 0.14s;
}

.ctx-pills:hover {
  background: var(--bg-hover);
}

.ctx-pill {
  display: inline-flex;
  align-items: center;
  min-width: 0;
  max-width: 180px;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 650;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ctx-paper {
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
}

.ctx-chevron {
  color: var(--text-tertiary);
  transition: transform 0.16s ease;
  flex-shrink: 0;
}

.ctx-chevron.open {
  transform: rotate(180deg);
}

.ctx-preview {
  width: min(760px, 100%);
  margin-top: 5px;
  border: 1px solid var(--border-subtle);
  border-radius: 10px;
  background: var(--bg-primary);
  overflow: hidden;
}

.ctx-section + .ctx-section {
  border-top: 1px solid var(--border-subtle);
}

.ctx-section-label {
  padding: 7px 12px 2px;
  font-size: 10px;
  font-weight: 650;
  color: var(--text-tertiary);
}

.ctx-preview-text {
  max-height: 260px;
  margin: 0;
  padding: 9px 12px 11px;
  overflow: auto;
  color: var(--text-secondary);
  font-family: inherit;
  font-size: 11.5px;
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}
.ctx-preview-empty {
  color: var(--text-tertiary);
  font-style: italic;
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

/* ── @ model picker (per-message) ─────────────────────────────────────────── */

.msg-model-picker { position: relative; }

.at-btn {
  width: 27px;
  height: 27px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-tertiary);
}
.at-btn:hover:not(:disabled),
.at-btn.active { color: var(--accent); background: color-mix(in srgb, var(--accent) 8%, transparent); }
.at-btn:disabled { opacity: 0.42; cursor: not-allowed; }

/* .msg-model-menu is no longer used (replaced by teleport) */

.msg-model-group + .msg-model-group {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--border-subtle);
}

.msg-model-group-name {
  padding: 2px 8px 5px;
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary);
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.msg-model-row {
  width: 100%;
  padding: 7px 10px;
  border-radius: 8px;
  color: var(--text-secondary);
  font-size: 13px;
  text-align: left;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.msg-model-row:hover { background: var(--bg-hover); color: var(--text-primary); }

/* ── Meta row: variant tabs + sources on same line ─────────────────────────── */

.meta-row {
  display: flex;
  flex-direction: column;
  margin-top: 4px;
}

.meta-divider {
  height: 1px;
  background: var(--border-subtle);
  margin-bottom: 8px;
}

.meta-content {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.answer-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  flex: 1;
  min-width: 0;
}

.answer-tab {
  width: 30px;
  height: 30px;
  border-radius: 9px;
  border: 2px solid transparent;
  background: var(--bg-secondary);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  flex-shrink: 0;
  transition: border-color 0.12s, box-shadow 0.12s;
}

.answer-tab img { width: 20px; height: 20px; object-fit: contain; border-radius: 4px; }

.answer-tab:hover {
  border-color: var(--border-default);
}

.answer-tab.active {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
}

.tab-icon-fallback {
  font-size: 11px;
  font-weight: 700;
  color: var(--text-secondary);
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

.st-snippet {
  background: color-mix(in srgb, #8b5cf6 14%, transparent);
  color: #8b5cf6;
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
  font-size: 16px;
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

.attach-btn {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 8px;
  color: var(--text-tertiary);
  background: transparent;
  flex-shrink: 0;
  cursor: pointer;
  transition: background .12s ease, color .12s ease;
}
.attach-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.attach-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.attachment-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 4px 0 8px;
}
.attachment-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 180px;
  padding: 4px 6px;
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  font-size: 12px;
  color: var(--text-primary);
}
.attachment-chip.pdf {
  background: #fff0f0;
  border-color: #f0c0c0;
  color: #8b1e1e;
}
.attachment-thumb {
  width: 18px;
  height: 18px;
  object-fit: cover;
  border-radius: 4px;
  flex-shrink: 0;
}
.attachment-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.attachment-remove {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  padding: 0;
  border: 0;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.attachment-remove:hover {
  background: rgba(0, 0, 0, 0.08);
  color: var(--text-primary);
}

.user-attachments {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 6px;
  max-width: 88%;
  justify-content: flex-end;
}
.user-attachment {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: 160px;
  padding: 4px 7px;
  border-radius: var(--radius-pill);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  color: var(--text-secondary);
  font-size: 11px;
  cursor: pointer;
}
.user-attachment.pdf {
  background: #fff0f0;
  border-color: #f0c0c0;
  color: #8b1e1e;
}
.user-attachment-thumb {
  width: 16px;
  height: 16px;
  object-fit: cover;
  border-radius: 3px;
  flex-shrink: 0;
}
.user-attachment-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.attachment-lightbox {
  position: fixed;
  inset: 0;
  z-index: 10000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.72);
  backdrop-filter: blur(2px);
}
.lightbox-image {
  max-width: 92vw;
  max-height: 92vh;
  border-radius: var(--radius-md);
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
}
.lightbox-pdf {
  width: 92vw;
  height: 92vh;
  border-radius: var(--radius-md);
  background: #fff;
}
.lightbox-close {
  position: absolute;
  top: 16px;
  right: 16px;
  width: 36px;
  height: 36px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 0;
  border-radius: var(--radius-pill);
  background: rgba(0, 0, 0, 0.45);
  color: #fff;
  cursor: pointer;
}
.lightbox-close:hover { background: rgba(0, 0, 0, 0.65); }

.add-paper-context-btn {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 3px;
  border: none;
  border-radius: 8px;
  color: var(--text-tertiary);
  background: transparent;
  flex-shrink: 0;
  cursor: pointer;
  transition: background .12s ease, color .12s ease;
}
.add-paper-context-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
/* When papers are selected the button widens to show the count and tints to the
   accent — a subtle state, not a solid fill, so the toolbar stays flat. */
.add-paper-context-btn.has-count {
  width: auto;
  padding: 0 8px;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
}
.add-paper-context-btn.has-count:hover {
  background: color-mix(in srgb, var(--accent) 16%, transparent);
}
.add-paper-context-btn .paper-count {
  font-size: 11px;
  font-weight: 700;
  line-height: 1;
}

/* Knowledge source picker */
.ks-picker {
  position: relative;
}

.ks-trigger {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 8px;
  border-radius: 8px;
  background: transparent;
  border: none;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  cursor: pointer;
  transition: background 0.12s ease, color 0.12s ease;
  white-space: nowrap;
}
.ks-trigger:hover,
.ks-trigger.active {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.ks-trigger.on {
  color: var(--accent);
}

.ks-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-tertiary);
  flex-shrink: 0;
}
.ks-trigger.on .ks-dot { background: var(--accent); }

/* ── Reasoning / thinking-mode picker (flat toolbar button + popover) ──────── */
.toolbar-btn {
  position: relative;
  width: 28px;
  height: 28px;
  border: none;
  background: transparent;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  flex-shrink: 0;
  cursor: pointer;
  transition: background .12s ease, color .12s ease;
}
.toolbar-btn:hover { color: var(--text-primary); background: var(--bg-hover); }
.toolbar-btn:disabled { opacity: .4; cursor: not-allowed; }
.toolbar-btn-active { color: var(--accent) !important; background: color-mix(in srgb, var(--accent) 8%, transparent) !important; }
.reasoning-badge {
  position: absolute;
  top: 1px;
  right: 1px;
  font-size: 8px;
  font-weight: 700;
  line-height: 1.4;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 15%, var(--bg-primary));
  border-radius: 3px;
  padding: 0 2px;
}
.reasoning-picker { position: relative; }
.reasoning-popover {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 90;
  min-width: 160px;
  padding: 10px;
  border-radius: 12px;
  border: 1px solid color-mix(in srgb, var(--border-default) 85%, transparent);
  background: color-mix(in srgb, var(--bg-primary) 96%, transparent);
  backdrop-filter: blur(18px) saturate(1.4);
  -webkit-backdrop-filter: blur(18px) saturate(1.4);
  box-shadow: 0 10px 32px rgba(0, 0, 0, .18);
}
.reasoning-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 8px;
}
.reasoning-label {
  font-size: 12px;
  font-weight: 650;
  color: var(--text-primary);
}
.reasoning-toggle {
  position: relative;
  width: 34px;
  height: 20px;
  border: none;
  border-radius: 10px;
  background: var(--border-default);
  transition: background .2s ease;
  flex-shrink: 0;
  cursor: pointer;
}
.reasoning-toggle.on { background: var(--accent); }
.toggle-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, .2);
  transition: transform .2s ease;
}
.reasoning-toggle.on .toggle-knob { transform: translateX(14px); }
.reasoning-levels {
  display: flex;
  gap: 5px;
}
.level-btn {
  flex: 1;
  padding: 4px 6px;
  border-radius: 7px;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  border: 1px solid var(--border-default);
  background: transparent;
  cursor: pointer;
  transition: all .15s ease;
}
.level-btn:hover { color: var(--accent); border-color: var(--accent); }
.level-btn.active {
  color: var(--accent);
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
}
.reasoning-drop-enter-active,
.reasoning-drop-leave-active { transition: opacity .15s ease, transform .15s ease; }
.reasoning-drop-enter-from,
.reasoning-drop-leave-to { opacity: 0; transform: translateY(4px); }

.ks-chevron {
  flex-shrink: 0;
  color: var(--text-tertiary);
  transition: transform 0.15s;
}
.ks-chevron.open { transform: rotate(180deg); }

.ks-menu {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  padding: 4px;
  min-width: 150px;
  z-index: 200;
}

.ks-option {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-primary);
  text-align: left;
  transition: background 0.08s;
}
.ks-option:hover { background: var(--bg-hover); }
.ks-option.selected { color: var(--accent); }
.ks-option svg { flex-shrink: 0; color: var(--text-tertiary); }
.ks-option.selected svg:first-child { color: var(--accent); }

.ks-option-text {
  flex: 1;
  min-width: 0;
}
.ks-option-hint {
  font-size: 10px;
  color: var(--text-tertiary);
  font-weight: 400;
  margin-left: 4px;
}
.ks-check { flex-shrink: 0; color: var(--accent); }

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

.stop-btn {
  background: color-mix(in srgb, var(--text-primary) 10%, transparent);
  color: var(--text-primary);
}
.stop-btn:hover { background: color-mix(in srgb, var(--text-primary) 16%, transparent); }

.paper-context-counter {
  height: 24px;
  display: inline-flex;
  align-items: center;
  padding: 0 8px;
  border-radius: var(--radius-pill);
  border: 1px solid var(--border-default);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 650;
  white-space: nowrap;
}

.paper-picker-overlay {
  position: fixed;
  inset: 0;
  z-index: 9500;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(15, 23, 42, 0.22);
}
.paper-picker-dialog {
  width: min(520px, calc(100vw - 40px));
  max-height: min(620px, calc(100vh - 80px));
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border-default);
  border-radius: 14px;
  background: var(--bg-primary);
  box-shadow: 0 24px 70px rgba(15, 23, 42, 0.22);
}
.paper-picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px 10px;
}
.paper-picker-title {
  font-size: 14px;
  font-weight: 650;
  color: var(--text-primary);
}
.paper-picker-close {
  width: 26px;
  height: 26px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  color: var(--text-tertiary);
}
.paper-picker-close:hover { background: var(--bg-hover); color: var(--text-primary); }
.paper-picker-search {
  margin: 0 16px 10px;
  padding: 8px 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 13px;
}
.paper-picker-search:focus {
  outline: none;
  border-color: var(--accent);
  background: var(--bg-primary);
}
.paper-picker-list {
  min-height: 0;
  overflow-y: auto;
  padding: 0 8px 12px;
}
.paper-picker-empty {
  padding: 24px;
  color: var(--text-tertiary);
  font-size: 13px;
  text-align: center;
}
.paper-picker-item {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 3px;
  width: 100%;
  padding: 10px 78px 10px 10px;
  border-radius: var(--radius-md);
  text-align: left;
  color: var(--text-primary);
}
.paper-picker-item:hover { background: var(--bg-hover); }
.paper-picker-item.selected {
  opacity: 0.62;
  cursor: default;
}
.paper-picker-item-title {
  font-size: 13px;
  font-weight: 600;
  line-height: 1.35;
}
.paper-picker-item-meta {
  font-size: 11px;
  color: var(--text-tertiary);
}
.paper-picker-badge {
  position: absolute;
  right: 10px;
  top: 50%;
  transform: translateY(-50%);
  font-size: 11px;
  color: var(--accent);
  font-weight: 650;
}

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
.markdown-body :deep(p:last-child) { margin-bottom: 0; }
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

<style>
/* Global: teleported model picker (scoped doesn't reach Teleport children) */
.msg-model-menu-teleport {
  position: fixed;
  transform: translateY(-100%);
  z-index: 9999;
  min-width: 220px;
  max-width: 320px;
  max-height: min(400px, 65vh);
  overflow-y: auto;
  padding: 6px;
  border: 1px solid var(--border-subtle);
  border-radius: 12px;
  background: var(--bg-primary);
  box-shadow: 0 12px 36px rgba(15,23,42,0.18);
}
.msg-model-menu-teleport .msg-model-group + .msg-model-group {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--border-subtle);
}
.msg-model-menu-teleport .msg-model-group-name {
  padding: 2px 8px 5px;
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary);
  letter-spacing: 0.04em;
  text-transform: uppercase;
}
.msg-model-menu-teleport .msg-model-row {
  display: block;
  width: 100%;
  padding: 7px 10px;
  border-radius: 8px;
  color: var(--text-secondary);
  font-size: 13px;
  text-align: left;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
  background: none;
  border: none;
}
.msg-model-menu-teleport .msg-model-row:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
</style>
