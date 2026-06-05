<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useAiStore, type ModelOption } from '../../stores/ai'
import MermaidBlock from '../MermaidBlock.vue'
import { renderMarkdown, getSegments } from '../../utils/renderMarkdown'
import { svgStringToPngBlob } from '../../utils/svgToPng'
import { copyPngBlobToClipboard } from '../../utils/clipboard'
import type { ChatMessage, PaperMeta, PaperStatus } from '../../types'
import { askAiText } from '../../stores/translationHistory'

const props = withDefaults(defineProps<{ slug: string | null; standalone?: boolean }>(), {
  standalone: false,
})
const emit = defineEmits<{ 'open-settings': [] }>()

const { t } = useI18n()
const ai = useAiStore()

type ChatNode =
  | { id: string; role: 'user'; content: string; createdAt: string }
  | { id: string; role: 'assistantGroup'; promptId: string; answers: AssistantAnswer[]; createdAt: string }
type AssistantGroupNode = Extract<ChatNode, { role: 'assistantGroup' }>

interface AssistantAnswer {
  id: string
  providerId: string
  providerName: string
  modelId: string
  modelName: string
  content: string
  reasoningContent?: string
  withReasoning?: boolean
  createdAt: string
  startedAt?: number
  endedAt?: number
  streaming?: boolean
  error?: boolean
  errorText?: string
  tokenEstimate?: number
  contextMode?: string
  usedPdf?: boolean
  source?: 'chat' | 'metadataExtraction'
  // Actual content injected into the system prompt, received via -context event
  contextContent?: { metadata: string; summary: string; fulltext: string }
}

interface Conversation {
  id: string
  slug: string
  title: string
  createdAt: string
  updatedAt: string
  nodes: ChatNode[]
  source?: 'chat' | 'metadataExtraction'
}

interface StreamPayload {
  delta: string
  done: boolean
}

interface ExtractionProgressPayload {
  slug?: string
  ok?: boolean
}

const STORAGE_PREFIX = 'argus.paper-ai-conversations.v2'

const allSelectableModels = computed<ModelOption[]>(() =>
  ai.chatModels
)

const conversations = ref<Conversation[]>([])
const activeConversation = ref<Conversation | null>(null)
const input = ref('')
const messagesEl = ref<HTMLElement | null>(null)
const textareaEl = ref<HTMLTextAreaElement | null>(null)
const selectedModelKeys = ref<string[]>([])
const showModelMenu = ref(false)
const showHistory = ref(false)
const modelMenuRoot = ref<HTMLElement | null>(null)
const unlisteners = new Map<string, UnlistenFn>()
const fulltextReady = ref(false)
const fulltextChecking = ref(false)
const abstractAvailable = ref(false)
const activeAnswerTabs = ref<Record<string, string>>({})
let unlistenExtractionProgress: UnlistenFn | null = null
let unlistenMetaStart: UnlistenFn | null = null
let unlistenMetaDone: UnlistenFn | null = null
let _compositionEndedAt = 0

// Copy state for message actions
const copiedIds = ref(new Set<string>())

// Inline edit state for user messages
const editingNodeId = ref<string | null>(null)
const editingText = ref('')

async function copyText(id: string, text: string) {
  await navigator.clipboard.writeText(text).catch(() => {})
  copiedIds.value.add(id)
  setTimeout(() => { copiedIds.value.delete(id); copiedIds.value = new Set(copiedIds.value) }, 2000)
}

async function openPopupWindow() {
  if (!props.slug) return
  localStorage.setItem('argus:paper-ai-slug', props.slug)
  await invoke('open_paper_ai_window', { slug: props.slug })
}

// Reasoning / thinking mode state
const useReasoning = ref(false)
const reasoningLevel = ref<'low' | 'medium' | 'high'>('high')
const reasoningOpen = ref(false)
const reasoningRoot = ref<HTMLElement | null>(null)

// Context mode: how much paper content to inject as system prompt context
// Possible values: 'none' | 'metadata' | 'summary' | 'fulltext' | 'summary+fulltext'
// All options are independent toggles; none = no context injected.
const contextMode = ref<'none' | 'metadata' | 'summary' | 'fulltext' | 'summary+fulltext'>('none')
const usePdf = ref(false)
const summaryAvailable = ref(false)

// PDF mode is only supported by OpenRouter providers
const pdfSupported = computed(() =>
  selectedModels.value.some(m =>
    ai.settings.providers.find(p => p.id === m.providerId)?.kind === 'openrouter'
  )
)

const hasSummary = computed(() =>
  contextMode.value === 'summary' || contextMode.value === 'summary+fulltext'
)
const hasFulltext = computed(() =>
  contextMode.value === 'fulltext' || contextMode.value === 'summary+fulltext'
)

const effectiveContextMode = computed(() => {
  // Fall back when fulltext not ready
  if (hasFulltext.value && !fulltextReady.value) {
    return hasSummary.value ? 'summary' : 'none'
  }
  return contextMode.value
})

const modelSvgModules = import.meta.glob<{ default: string }>('/src/assets/models/*.svg', { eager: true })
const modelIconMap: Record<string, string> = {}
for (const [path, mod] of Object.entries(modelSvgModules)) {
  modelIconMap[path.replace(/^.*\//, '').replace(/\.svg$/, '')] = mod.default
}

const selectedModels = computed(() =>
  selectedModelKeys.value
    .map(k => modelByKey(k))
    .filter((m): m is ModelOption => !!m)
)

const hasStreaming = computed(() =>
  !!activeConversation.value?.nodes.some(n =>
    n.role === 'assistantGroup' && n.answers.some(a => a.streaming)
  )
)

// True if the primary selected model's provider has a DeepSeek endpoint
const isDeepSeekSelected = computed(() => {
  const primary = selectedModels.value[0]
  if (!primary) return false
  const provider = ai.settings.providers.find(p => p.id === primary.providerId)
  return !!provider?.base_url.toLowerCase().includes('deepseek')
})

// Access an answer through Vue's reactive proxy chain (fixes reactivity bug)
function findReactiveAnswer(answerId: string): AssistantAnswer | null {
  if (!activeConversation.value) return null
  for (const node of activeConversation.value.nodes) {
    if (node.role === 'assistantGroup') {
      const ans = node.answers.find(a => a.id === answerId)
      if (ans) return ans
    }
  }
  return null
}

function isMetadataExtractionConversation(conv: Conversation | null) {
  if (!conv) return false
  if (conv.source === 'metadataExtraction' || conv.title === 'AI 元数据提取') return true
  const firstUser = conv.nodes.find((node): node is Extract<ChatNode, { role: 'user' }> => node.role === 'user')
  const prompt = firstUser?.content.trim() ?? ''
  return /^Extract academic paper metadata\b/i.test(prompt)
}

const activeConversationIsMetadataExtraction = computed(() =>
  isMetadataExtractionConversation(activeConversation.value)
)

const activeTitle = computed(() => {
  if (activeConversationIsMetadataExtraction.value) return 'AI 元数据提取'
  const title = activeConversation.value?.title?.trim()
  return title && title !== '新对话' ? title : '新对话'
})

const canSend = computed(() =>
  !!props.slug && input.value.trim().length > 0 && selectedModels.value.length > 0 && !hasStreaming.value
)

function storageKey(slug: string) {
  return `${STORAGE_PREFIX}:${slug}`
}

function nowIso() {
  return new Date().toISOString()
}

function newId(prefix: string) {
  return `${prefix}_${crypto.randomUUID()}`
}

function cloneConversation(conv: Conversation): Conversation {
  return JSON.parse(JSON.stringify(conv)) as Conversation
}

function createBlankConversation(slug: string): Conversation {
  const ts = nowIso()
  return { id: newId('conv'), slug, title: '新对话', createdAt: ts, updatedAt: ts, nodes: [] }
}

function normalizeConversations(value: unknown, slug: string): Conversation[] {
  if (!Array.isArray(value)) return []
  return value
    .filter((c): c is Conversation =>
      !!c &&
      typeof c === 'object' &&
      Array.isArray((c as Conversation).nodes) &&
      (!(c as Conversation).slug || (c as Conversation).slug === slug)
    )
    .map(c => ({
      ...c,
      slug,
      title: c.title || '新对话',
      createdAt: c.createdAt || nowIso(),
      updatedAt: c.updatedAt || c.createdAt || nowIso(),
    }))
    .sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime())
}

function persistedConversations(): Conversation[] {
  return conversations.value
    .filter(c => c.nodes.length > 0)
    .sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime())
    .map(cloneConversation)
}

async function saveConversationsToPaper(slug: string) {
  const persisted = persistedConversations()
  conversations.value = persisted
  await invoke('save_paper_ai_conversations', { slug, conversations: persisted })
  try {
    localStorage.removeItem(storageKey(slug))
  } catch {
    // non-fatal cleanup of the old browser cache
  }
}

async function importLegacyHistory(slug: string) {
  if (conversations.value.length > 0) return
  try {
    const history = await invoke<ChatMessage[]>('get_chat_history', { slug })
    const nodes: ChatNode[] = []
    for (const msg of history.filter(m => m.role !== 'system' && m.content?.trim())) {
      if (msg.role === 'user') {
        nodes.push({ id: newId('user'), role: 'user', content: msg.content, createdAt: nowIso() })
      } else if (msg.role === 'assistant') {
        const fallback = ai.chatModels[0]
        nodes.push({
          id: newId('group'),
          role: 'assistantGroup',
          promptId: nodes[nodes.length - 1]?.id ?? '',
          createdAt: nowIso(),
          answers: [{
            id: newId('answer'),
            providerId: fallback?.providerId ?? '',
            providerName: fallback?.providerName ?? 'AI',
            modelId: fallback?.modelId ?? '',
            modelName: fallback?.displayName ?? 'AI',
            content: msg.content,
            createdAt: nowIso(),
            tokenEstimate: estimateTokens(msg.content),
          }],
        })
      }
    }
    if (!nodes.length) return
    const ts = nowIso()
    conversations.value = [{
      id: newId('conv'),
      slug,
      title: firstUserTitle(nodes) || '历史对话',
      createdAt: ts,
      updatedAt: ts,
      nodes,
    }]
    await saveConversationsToPaper(slug)
  } catch {
    // legacy history is optional
  }
}

async function loadConversations(slug: string) {
  try {
    conversations.value = normalizeConversations(
      await invoke<unknown>('get_paper_ai_conversations', { slug }),
      slug
    )
  } catch {
    conversations.value = []
  }
  await importLegacyHistory(slug)
  startNewConversation(false)
}

function applyFulltextReady(ready: boolean, resetMode = false) {
  fulltextReady.value = ready
  if (resetMode) {
    contextMode.value = 'none'
  } else if (!ready && hasFulltext.value) {
    // Remove fulltext from mode; keep summary if active
    contextMode.value = hasSummary.value ? 'summary' : 'none'
  }
}

async function refreshSummaryAvailability(slug = props.slug) {
  if (!slug) { summaryAvailable.value = false; return }
  try {
    const status = await invoke<{ ai_summary_done: boolean }>('get_paper_status', { slug })
    if (props.slug === slug) summaryAvailable.value = !!status.ai_summary_done
  } catch {
    if (props.slug === slug) summaryAvailable.value = false
  }
}

async function refreshAbstractAvailability(slug = props.slug) {
  if (!slug) { abstractAvailable.value = false; return }
  try {
    const meta = await invoke<PaperMeta>('get_paper_meta', { slug })
    if (props.slug === slug) abstractAvailable.value = !!meta.abstract?.trim()
  } catch {
    if (props.slug === slug) abstractAvailable.value = false
  }
}

async function refreshFulltextAvailability(slug = props.slug, resetMode = false) {
  if (!slug) {
    applyFulltextReady(false, resetMode)
    fulltextChecking.value = false
    return
  }
  fulltextChecking.value = true
  try {
    const status = await invoke<PaperStatus>('get_paper_status', { slug })
    let ready = !!status.text_extracted
    if (!ready) {
      const text = await invoke<string>('get_fulltext', { slug })
      ready = text.trim().length > 0
    }
    if (props.slug === slug) applyFulltextReady(ready, resetMode)
  } catch {
    if (props.slug === slug) applyFulltextReady(false, resetMode)
  } finally {
    if (props.slug === slug) fulltextChecking.value = false
  }
}

function toggleContext(option: 'metadata' | 'summary' | 'fulltext') {
  if (option === 'metadata') {
    // Toggle metadata: deselect if already active (clears all), select if not active
    contextMode.value = contextMode.value === 'metadata' ? 'none' : 'metadata'
    return
  }
  if (option === 'fulltext') {
    if (!fulltextReady.value) return
    if (hasFulltext.value) {
      contextMode.value = hasSummary.value ? 'summary' : 'none'
    } else {
      // Selecting fulltext clears metadata
      contextMode.value = hasSummary.value ? 'summary+fulltext' : 'fulltext'
    }
    return
  }
  // option === 'summary'
  if (!summaryAvailable.value) return
  if (hasSummary.value) {
    contextMode.value = hasFulltext.value ? 'fulltext' : 'none'
  } else {
    // Selecting summary clears metadata
    contextMode.value = hasFulltext.value ? 'summary+fulltext' : 'summary'
  }
}

function onWindowFocus() {
  if (props.slug) {
    refreshFulltextAvailability(props.slug).catch(() => {})
    refreshSummaryAvailability(props.slug).catch(() => {})
  }
}

function onPaperFulltextUpdated(event: Event) {
  const slug = (event as CustomEvent<{ slug?: string }>).detail?.slug
  if (slug && slug === props.slug) applyFulltextReady(true)
}

function onPaperMetaUpdated(event: Event) {
  const { slug, meta } = (event as CustomEvent<{ slug?: string; meta?: PaperMeta }>).detail ?? {}
  if (slug && slug === props.slug && meta) abstractAvailable.value = !!meta.abstract?.trim()
}

function startNewConversation(closeHistory = true) {
  if (!props.slug) return
  activeConversation.value = createBlankConversation(props.slug)
  activeAnswerTabs.value = {}
  if (closeHistory) showHistory.value = false
  nextTick(() => textareaEl.value?.focus())
}

watch(askAiText, (text) => {
  if (text === null) return
  startNewConversation(true)
  input.value = text + '\n\n'
  nextTick(() => {
    const el = textareaEl.value
    if (!el) return
    el.focus()
    el.setSelectionRange(el.value.length, el.value.length)
  })
}, { flush: 'post' })

function openConversation(id: string) {
  const conv = conversations.value.find(c => c.id === id)
  if (!conv) return
  activeConversation.value = cloneConversation(conv)
  activeAnswerTabs.value = {}
  showHistory.value = false
  nextTick(() => {
    scrollToBottom(true)
    textareaEl.value?.focus()
  })
}

function deleteConversation(id: string, event?: MouseEvent) {
  event?.stopPropagation()
  if (!props.slug) return
  conversations.value = conversations.value.filter(c => c.id !== id)
  if (activeConversation.value?.id === id) startNewConversation(false)
  saveConversationsToPaper(props.slug).catch(() => {})
}

function persistActiveConversation() {
  const conv = activeConversation.value
  if (!props.slug || !conv || conv.nodes.length === 0) return
  conv.updatedAt = nowIso()
  conv.title = isMetadataExtractionConversation(conv)
    ? 'AI 元数据提取'
    : firstUserTitle(conv.nodes) || conv.title || '新对话'
  const idx = conversations.value.findIndex(c => c.id === conv.id)
  if (idx >= 0) conversations.value[idx] = cloneConversation(conv)
  else conversations.value.unshift(cloneConversation(conv))
  conversations.value.sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime())
  saveConversationsToPaper(props.slug).catch(() => {})
  saveLegacyActiveHistory(conv).catch(() => {})
}

async function saveLegacyActiveHistory(conv: Conversation) {
  if (!props.slug) return
  const messages = flattenConversation(conv)
  await invoke('save_chat_history', { slug: props.slug, messages })
}

function firstUserTitle(nodes: ChatNode[]) {
  const first = nodes.find(n => n.role === 'user')
  if (!first || first.role !== 'user') return ''
  const title = first.content.replace(/\s+/g, ' ').trim()
  return title.length > 32 ? `${title.slice(0, 32)}…` : title
}

function activeAnswerForGroup(group: AssistantGroupNode) {
  const activeId = activeAnswerTabs.value[group.id]
  return group.answers.find(answer => answer.id === activeId) ?? group.answers[0] ?? null
}

function activeAnswersForGroup(group: AssistantGroupNode) {
  const answer = activeAnswerForGroup(group)
  return answer ? [answer] : []
}

function setActiveAnswer(groupId: string, answerId: string) {
  activeAnswerTabs.value = { ...activeAnswerTabs.value, [groupId]: answerId }
}

function answerModelLabel(answer: AssistantAnswer) {
  return [answer.providerName, answer.modelName].filter(Boolean).join(' · ')
}

function modelKey(model: Pick<ModelOption, 'providerId' | 'modelId'>) {
  return `${model.providerId}::${model.modelId}`
}

function modelByKey(key: string): ModelOption | null {
  const sep = key.indexOf('::')
  if (sep < 0) return null
  const providerId = key.slice(0, sep)
  const modelId = key.slice(sep + 2)
  return allSelectableModels.value.find(m => m.providerId === providerId && m.modelId === modelId) ?? null
}

const LAST_MODEL_KEY = 'argus:ai-last-model'

function ensureDefaultModels() {
  if (!allSelectableModels.value.length) {
    selectedModelKeys.value = []
    return
  }
  const valid = selectedModelKeys.value.filter(k => !!modelByKey(k))
  if (valid.length) {
    selectedModelKeys.value = valid
    return
  }
  // Restore last-used model from localStorage
  const saved = localStorage.getItem(LAST_MODEL_KEY)
  if (saved && modelByKey(saved)) {
    selectedModelKeys.value = [saved]
    return
  }
  const def = ai.defaultSelection
    ? allSelectableModels.value.find(m => m.providerId === ai.defaultSelection?.providerId && m.modelId === ai.defaultSelection?.modelId)
    : null
  selectedModelKeys.value = [modelKey(def ?? allSelectableModels.value[0])]
}

function toggleModel(model: ModelOption) {
  selectedModelKeys.value = [modelKey(model)]
  localStorage.setItem(LAST_MODEL_KEY, modelKey(model))
  showModelMenu.value = false
}

function selectOnly(model: ModelOption) {
  selectedModelKeys.value = [modelKey(model)]
  localStorage.setItem(LAST_MODEL_KEY, modelKey(model))
  showModelMenu.value = false
}

function selectedModelLabel() {
  if (selectedModels.value.length === 0) return '选择模型'
  if (selectedModels.value.length === 1) return selectedModels.value[0].displayName
  return `${selectedModels.value.length} 个模型`
}

function modelLogo(modelId: string, providerName = '', providerId = '') {
  const haystack = `${modelId} ${providerName} ${providerId}`.toLowerCase()
  if (haystack.includes('deepseek')) return modelIconMap.deepseek
  if (haystack.includes('claude') || haystack.includes('anthropic')) return modelIconMap.claude
  if (haystack.includes('gemini') || haystack.includes('google')) return modelIconMap.gemini
  if (haystack.includes('qwen') || haystack.includes('通义') || haystack.includes('alibaba')) return modelIconMap.qwen ?? modelIconMap.alibaba
  if (haystack.includes('kimi') || haystack.includes('moonshot')) return modelIconMap.kimi
  if (haystack.includes('grok') || haystack.includes('xai')) return modelIconMap.grok ?? modelIconMap.xai
  if (haystack.includes('ollama')) return modelIconMap['ollama-color']
  if (haystack.includes('zhipu') || haystack.includes('智谱') || haystack.includes('glm')) return modelIconMap.zhipu
  if (haystack.includes('baidu') || haystack.includes('ernie')) return modelIconMap.baidu
  if (haystack.includes('doubao') || haystack.includes('bytedance')) return modelIconMap.bytedance
  if (haystack.includes('mistral') || haystack.includes('huggingface')) return modelIconMap.huggingface
  if (haystack.includes('gpt') || haystack.includes('openai')) return modelIconMap.openai
  for (const key of Object.keys(modelIconMap)) {
    if (haystack.includes(key)) return modelIconMap[key]
  }
  return ''
}

function capabilitiesLabel(model: ModelOption) {
  const caps = model.capabilities ?? []
  const labels: string[] = []
  if (caps.some(c => /vision|image/i.test(c))) labels.push('视觉')
  if (caps.some(c => /reason|thinking/i.test(c))) labels.push('推理')
  if (model.contextLength) labels.push(formatContext(model.contextLength))
  return labels
}

function formatContext(n: number) {
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)}M`
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`
  return String(n)
}

function buildHistoryUntil(conv: Conversation, stopGroupId?: string): ChatMessage[] {
  const messages: ChatMessage[] = []
  for (const node of conv.nodes) {
    if (node.role === 'assistantGroup') {
      if (node.id === stopGroupId) break
      const answer = node.answers.find(a => !a.error && a.content.trim()) ?? node.answers.find(a => a.content.trim())
      if (answer) messages.push({ role: 'assistant', content: answer.content })
    } else {
      messages.push({ role: 'user', content: node.content })
    }
  }
  return messages
}

function flattenConversation(conv: Conversation): ChatMessage[] {
  return buildHistoryUntil(conv)
}

async function sendMessage() {
  if (!canSend.value || !props.slug) return
  const text = input.value.trim()
  const conv = activeConversation.value ?? createBlankConversation(props.slug)
  activeConversation.value = conv

  const userNode: ChatNode = { id: newId('user'), role: 'user', content: text, createdAt: nowIso() }
  const group: ChatNode = {
    id: newId('group'),
    role: 'assistantGroup',
    promptId: userNode.id,
    createdAt: nowIso(),
    answers: selectedModels.value.map(modelToAnswer),
  }
  if (group.answers[0]) setActiveAnswer(group.id, group.answers[0].id)
  conv.nodes.push(userNode, group)
  conv.title = firstUserTitle(conv.nodes) || conv.title
  input.value = ''
  persistActiveConversation()
  await nextTick()
  resizeTextarea()
  scrollToBottom(true)

  const history = buildHistoryUntil(conv, group.id)
  await Promise.all(group.answers.map(answer => streamAnswer(conv, answer, history)))
}

function modelToAnswer(model: ModelOption): AssistantAnswer {
  return {
    id: newId('answer'),
    providerId: model.providerId,
    providerName: model.providerName,
    modelId: model.modelId,
    modelName: model.displayName,
    content: '',
    withReasoning: useReasoning.value,
    createdAt: nowIso(),
    contextMode: effectiveContextMode.value,
    usedPdf: usePdf.value && pdfSupported.value,
  }
}

async function regenerate(group: ChatNode, answer: AssistantAnswer) {
  if (group.role !== 'assistantGroup' || !activeConversation.value || !props.slug || answer.streaming) return
  const ra = findReactiveAnswer(answer.id)
  if (ra) {
    ra.content = ''
    ra.reasoningContent = ''
    ra.error = false
    ra.errorText = ''
    ra.tokenEstimate = 0
    ra.createdAt = nowIso()
  }
  persistActiveConversation()
  const history = buildHistoryUntil(activeConversation.value, group.id)
  await streamAnswer(activeConversation.value, answer, history)
}

type UserNode = Extract<ChatNode, { role: 'user' }>

function startEdit(node: UserNode) {
  if (hasStreaming.value) return
  editingNodeId.value = node.id
  editingText.value = node.content
}

function cancelEdit() {
  editingNodeId.value = null
  editingText.value = ''
}

async function submitEdit(node: UserNode) {
  const conv = activeConversation.value
  if (!conv || !props.slug || hasStreaming.value) return
  const newText = editingText.value.trim()
  if (!newText) return

  node.content = newText
  editingNodeId.value = null
  editingText.value = ''

  // Truncate everything after this user node
  const idx = conv.nodes.indexOf(node)
  if (idx >= 0) conv.nodes.splice(idx + 1)

  const group: ChatNode = {
    id: newId('group'),
    role: 'assistantGroup',
    promptId: node.id,
    createdAt: nowIso(),
    answers: selectedModels.value.map(modelToAnswer),
  }
  if (group.answers[0]) setActiveAnswer(group.id, group.answers[0].id)
  conv.nodes.push(group)
  conv.title = firstUserTitle(conv.nodes) || conv.title
  persistActiveConversation()
  await nextTick()
  scrollToBottom(true)

  const history = buildHistoryUntil(conv, group.id)
  await Promise.all(group.answers.map(answer => streamAnswer(conv, answer, history)))
}

function onEditKeydown(e: KeyboardEvent, node: UserNode) {
  if (e.key === 'Escape') { cancelEdit(); return }
  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submitEdit(node) }
}

function stopAllStreaming() {
  for (const [key, off] of unlisteners.entries()) {
    off()
    unlisteners.delete(key)
    const answerId = key.replace(/-reasoning$/, '')
    const ra = findReactiveAnswer(answerId)
    if (ra?.streaming) {
      ra.streaming = false
      ra.endedAt = performance.now()
    }
  }
  persistActiveConversation()
}

async function streamAnswer(conv: Conversation, answer: AssistantAnswer, history: ChatMessage[]) {
  if (!props.slug) return
  const eventName = `paper-ai-chat-${answer.id}`
  const reasoningEventName = `${eventName}-reasoning`

  // Initialize through reactive proxy chain so Vue tracks all mutations
  const ra = findReactiveAnswer(answer.id)
  if (ra) {
    ra.streaming = true
    ra.error = false
    ra.errorText = ''
    ra.content = ''
    ra.reasoningContent = ''
    ra.startedAt = performance.now()
    ra.endedAt = undefined
    ra.tokenEstimate = 0
  }

  const unlisten = await listen<StreamPayload>(eventName, (event) => {
    if (event.payload.done) return
    const reactiveAns = findReactiveAnswer(answer.id)
    if (!reactiveAns) return
    reactiveAns.content += event.payload.delta
    reactiveAns.tokenEstimate = estimateTokens(reactiveAns.content)
    scrollToBottom()
  })
  unlisteners.set(answer.id, unlisten)

  // Receive the actual context injected into the system prompt for the transparency banner
  const unlistenCtx = await listen<{ metadata: string; summary: string; fulltext: string }>(
    `${eventName}-context`,
    (event) => {
      const reactiveAns = findReactiveAnswer(answer.id)
      if (reactiveAns) reactiveAns.contextContent = event.payload
    },
  )
  unlisteners.set(`${answer.id}-context`, unlistenCtx)

  // Only listen to reasoning events when the user explicitly enabled the toggle.
  // Some models (e.g. DeepSeek) emit reasoning_content by default; suppress it here
  // so "思考过程" never appears unless the user opted in.
  if (useReasoning.value) {
    const unlistenReasoning = await listen<StreamPayload>(reasoningEventName, (event) => {
      if (event.payload.done) return
      const reactiveAns = findReactiveAnswer(answer.id)
      if (!reactiveAns) return
      reactiveAns.reasoningContent = (reactiveAns.reasoningContent ?? '') + event.payload.delta
    })
    unlisteners.set(`${answer.id}-reasoning`, unlistenReasoning)
  }

  // Determine reasoning effort for DeepSeek: medium->high, high->max (mapped in backend)
  const providerInfo = ai.settings.providers.find(p => p.id === answer.providerId)
  const isDeepseek = !!providerInfo?.base_url.toLowerCase().includes('deepseek')
  // For DeepSeek: reasoningLevel 'low'/'medium' -> backend maps to 'high', 'high' -> 'max'
  const effortToSend = isDeepseek
    ? (reasoningLevel.value === 'high' ? 'high' : 'medium')  // backend does the high/max mapping
    : reasoningLevel.value

  try {
    const finalText = await invoke<string>('chat_with_paper_event', {
      slug: props.slug,
      messages: history,
      providerId: answer.providerId || null,
      modelId: answer.modelId || null,
      eventName,
      useReasoning: useReasoning.value,
      reasoningEffort: useReasoning.value ? effortToSend : null,
      contextMode: effectiveContextMode.value,
      usePdf: usePdf.value,
    })
    const reactiveAns = findReactiveAnswer(answer.id)
    if (reactiveAns) {
      if (!reactiveAns.content && finalText) reactiveAns.content = finalText
      reactiveAns.tokenEstimate = estimateTokens(reactiveAns.content)
    }
  } catch (e) {
    const reactiveAns = findReactiveAnswer(answer.id)
    if (reactiveAns) {
      reactiveAns.error = true
      reactiveAns.errorText = String(e)
      reactiveAns.content = `生成失败：${String(e)}`
    }
  } finally {
    const reactiveAns = findReactiveAnswer(answer.id)
    if (reactiveAns) {
      reactiveAns.streaming = false
      reactiveAns.endedAt = performance.now()
    }
    const off = unlisteners.get(answer.id)
    if (off) off()
    unlisteners.delete(answer.id)
    const offR = unlisteners.get(`${answer.id}-reasoning`)
    if (offR) offR()
    unlisteners.delete(`${answer.id}-reasoning`)
    const offCtx = unlisteners.get(`${answer.id}-context`)
    if (offCtx) offCtx()
    unlisteners.delete(`${answer.id}-context`)
    persistActiveConversation()
    scrollToBottom()
  }
}

function estimateTokens(text: string) {
  if (!text) return 0
  const cjk = (text.match(/[\u4e00-\u9fff]/g) ?? []).length
  const other = Math.max(0, text.length - cjk)
  return Math.max(1, Math.round(cjk * 0.8 + other / 4))
}

function answerSpeed(answer: AssistantAnswer) {
  if (!answer.startedAt) return ''
  const end = answer.endedAt ?? performance.now()
  const seconds = Math.max(0.2, (end - answer.startedAt) / 1000)
  const speed = Math.round((answer.tokenEstimate ?? estimateTokens(answer.content)) / seconds)
  return speed > 0 ? `~${speed} tok/s` : ''
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

function resizeTextarea() {
  nextTick(() => {
    const el = textareaEl.value
    if (!el) return
    const lineH = parseFloat(getComputedStyle(el).lineHeight) || 19
    const padTop = parseFloat(getComputedStyle(el).paddingTop) || 10
    el.style.height = 'auto'
    const lines = Math.ceil((el.scrollHeight - padTop) / lineH)
    const maxLines = 6
    const snapped = Math.min(maxLines, lines) * lineH + padTop
    el.style.height = `${snapped}px`
  })
}

function onCompositionStart() { _compositionEndedAt = 0 }
function onCompositionEnd()   { _compositionEndedAt = Date.now() }
function isIMEActive()        { return Date.now() - _compositionEndedAt < 100 }

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey && !isIMEActive()) {
    e.preventDefault()
    sendMessage()
  }
}

function scrollToBottom(force = false) {
  nextTick(() => {
    const el = messagesEl.value
    if (!el) return
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 120
    if (force || nearBottom) el.scrollTop = el.scrollHeight
  })
}

function formatTime(iso: string) {
  const date = new Date(iso)
  const now = new Date()
  if (date.toDateString() === now.toDateString()) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  }
  return date.toLocaleDateString([], { month: '2-digit', day: '2-digit' })
}

function finaliseMetaAnswer(answerId: string) {
  const ra = findReactiveAnswer(answerId)
  if (ra) {
    ra.streaming = false
    ra.endedAt = performance.now()
    ra.tokenEstimate = estimateTokens(ra.content)
  }
  const off = unlisteners.get(answerId)
  off?.()
  unlisteners.delete(answerId)
  persistActiveConversation()
  scrollToBottom()
}

function closeFloating(e: MouseEvent) {
  if (modelMenuRoot.value && !modelMenuRoot.value.contains(e.target as Node)) {
    showModelMenu.value = false
  }
  if (reasoningRoot.value && !reasoningRoot.value.contains(e.target as Node)) {
    reasoningOpen.value = false
  }
}

watch(() => props.slug, async (slug) => {
  for (const off of unlisteners.values()) off()
  unlisteners.clear()
  showHistory.value = false
  showModelMenu.value = false
  activeConversation.value = null
  conversations.value = []
  activeAnswerTabs.value = {}
  input.value = ''
  applyFulltextReady(false, true)
  abstractAvailable.value = false
  summaryAvailable.value = false
  if (slug) {
    await Promise.all([
      refreshFulltextAvailability(slug, true),
      refreshAbstractAvailability(slug),
      refreshSummaryAvailability(slug),
    ])
    await loadConversations(slug)
  }
}, { immediate: true })

watch(() => allSelectableModels.value.map(modelKey).join('|'), ensureDefaultModels, { immediate: true })
watch(input, resizeTextarea)
watch(activeConversationIsMetadataExtraction, (isMetadataExtraction) => {
  if (isMetadataExtraction) showModelMenu.value = false
})

onMounted(async () => {
  if (!ai.loaded) await ai.load()
  ensureDefaultModels()
  document.addEventListener('mousedown', closeFloating)
  window.addEventListener('focus', onWindowFocus)
  window.addEventListener('argus-paper-fulltext-updated', onPaperFulltextUpdated)
  window.addEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  unlistenExtractionProgress = await listen<ExtractionProgressPayload>('extraction_progress', (event) => {
    if (event.payload.slug === props.slug && event.payload.ok) {
      applyFulltextReady(true)
    }
  })
  unlistenMetaStart = await listen<{
    slug: string; group_id: string; answer_id: string; prompt: string
    provider_id: string; provider_name: string; model_id: string
  }>('ai-meta-start', async (ev) => {
    if (ev.payload.slug !== props.slug) return
    const { group_id, answer_id, prompt, provider_id, provider_name, model_id } = ev.payload
    const ts = nowIso()
    const userId = `user_${group_id}`
    const userNode: ChatNode = { id: userId, role: 'user', content: prompt, createdAt: ts }
    const answer: AssistantAnswer = {
      id: answer_id,
      providerId: provider_id,
      providerName: provider_name,
      modelId: model_id,
      modelName: model_id,
      content: '',
      createdAt: ts,
      streaming: true,
      startedAt: performance.now(),
      source: 'metadataExtraction',
    }
    const groupNode: ChatNode = {
      id: group_id,
      role: 'assistantGroup',
      promptId: userId,
      createdAt: ts,
      answers: [answer],
    }
    const conv: Conversation = {
      id: newId('conv'),
      slug: props.slug!,
      title: 'AI 元数据提取',
      createdAt: ts,
      updatedAt: ts,
      nodes: [userNode, groupNode],
      source: 'metadataExtraction',
    }
    conversations.value.unshift(conv)
    activeConversation.value = conv

    // Wire up stream listener using the same event pattern as copilot
    const unlisten = await listen<StreamPayload>(`paper-ai-chat-${answer_id}`, (event) => {
      if (event.payload.done) return
      const ra = findReactiveAnswer(answer_id)
      if (!ra) return
      ra.content += event.payload.delta
      ra.tokenEstimate = estimateTokens(ra.content)
      scrollToBottom()
    })
    unlisteners.set(answer_id, unlisten)
    nextTick(() => scrollToBottom(true))
  })

  unlistenMetaDone = await listen<{ slug: string; answer_id: string; error?: string }>(
    'ai-meta-done',
    (ev) => {
      if (ev.payload.slug !== props.slug) return
      finaliseMetaAnswer(ev.payload.answer_id)
    }
  )
  // Also handle error event from Rust (streaming failed)
  listen<{ slug: string; answer_id: string; error: string }>('ai-meta-error', (ev) => {
    if (ev.payload.slug !== props.slug) return
    const ra = findReactiveAnswer(ev.payload.answer_id)
    if (ra) {
      ra.error = true
      ra.errorText = ev.payload.error
      if (!ra.content) ra.content = `提取失败：${ev.payload.error}`
    }
    finaliseMetaAnswer(ev.payload.answer_id)
  })
  messagesEl.value?.addEventListener('copy-code', (e: Event) => {
    navigator.clipboard.writeText((e.target as HTMLElement).textContent ?? '').catch(() => {})
  })
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeFloating)
  window.removeEventListener('focus', onWindowFocus)
  window.removeEventListener('argus-paper-fulltext-updated', onPaperFulltextUpdated)
  window.removeEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  unlistenExtractionProgress?.()
  unlistenMetaStart?.()
  unlistenMetaDone?.()
  for (const off of unlisteners.values()) off()
  unlisteners.clear()
})

// ── Context banner ────────────────────────────────────────────────────────────
const expandedContextId = ref<string | null>(null)

function getFirstAnswer(userNodeId: string): AssistantAnswer | undefined {
  const nodes = activeConversation.value?.nodes ?? []
  const group = nodes.find(
    n => n.role === 'assistantGroup' && (n as AssistantGroupNode).promptId === userNodeId,
  ) as AssistantGroupNode | undefined
  return group?.answers[0]
}

function hasContextBanner(userNodeId: string): boolean {
  const ans = getFirstAnswer(userNodeId)
  if (!ans) return false
  if (ans.contextContent) {
    return !!(ans.contextContent.metadata || ans.contextContent.summary || ans.contextContent.fulltext) || !!ans.usedPdf
  }
  // Fallback for old conversations without contextContent
  const mode = ans.contextMode ?? 'none'
  return (mode !== 'none' && mode !== '') || !!ans.usedPdf
}

function toggleContextPanel(nodeId: string) {
  expandedContextId.value = expandedContextId.value === nodeId ? null : nodeId
}
</script>

<template>
  <div class="paper-ai" :class="{ standalone: props.standalone }">
    <div v-if="!slug" class="center-hint">
      <p>{{ t('sidebar.selectPaper') }}</p>
    </div>

    <template v-else-if="!ai.isConfigured && ai.loaded">
      <div class="center-hint">
        <svg width="34" height="34" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6">
          <path d="M12 3 13.8 8.2 19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/>
          <path d="M19 15v4"/><path d="M17 17h4"/>
        </svg>
        <h3>{{ t('copilot.noProviderTitle') }}</h3>
        <p>{{ t('copilot.noProviderDesc') }}</p>
        <button class="primary-btn" @click="emit('open-settings')">{{ t('copilot.goToSettings') }}</button>
      </div>
    </template>

    <template v-else>
      <aside v-if="props.standalone" class="history-sidebar">
        <div class="history-sidebar-head">
          <span>历史记录</span>
          <span>{{ conversations.length }}</span>
        </div>

        <button class="new-chat-btn sidebar-new-chat" @click="startNewConversation(false)">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12 5v14"/><path d="M5 12h14"/>
          </svg>
          新对话
        </button>

        <div class="history-list sidebar-history-list">
          <div
            v-for="conv in conversations"
            :key="conv.id"
            class="history-item"
            :class="{ active: conv.id === activeConversation?.id }"
            @click="openConversation(conv.id)"
          >
            <span class="history-title">{{ conv.title }}</span>
            <span class="history-meta">{{ conv.nodes.filter(n => n.role === 'user').length }} 问 · {{ formatTime(conv.updatedAt) }}</span>
            <button class="history-delete" title="删除" @click="deleteConversation(conv.id, $event)">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/>
              </svg>
            </button>
          </div>
          <div v-if="conversations.length === 0" class="history-empty">暂无历史对话</div>
        </div>
      </aside>

      <section class="chat-main">
        <header class="ai-header" :data-tauri-drag-region="props.standalone ? '' : undefined">
          <div class="header-main" :data-tauri-drag-region="props.standalone ? '' : undefined">
            <span class="spark-icon">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 3 13.8 8.2 19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/>
                <path d="M19 15v4"/><path d="M17 17h4"/>
              </svg>
            </span>
            <span class="header-title">{{ activeTitle }}</span>
          </div>

          <div class="header-actions">
            <button v-if="!props.standalone" class="icon-btn" title="在独立窗口打开" @click="openPopupWindow">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
              </svg>
            </button>
            <button v-if="!props.standalone" class="icon-btn" title="对话历史" @click="showHistory = true">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
                <path d="M8 8h8"/><path d="M8 12h5"/>
              </svg>
            </button>
          </div>
        </header>

        <div v-if="!activeConversationIsMetadataExtraction" ref="modelMenuRoot" class="model-picker floating-model-picker">
          <button class="model-trigger" @click.stop="showModelMenu = !showModelMenu">
            <span class="model-trigger-icon">
              <template v-if="selectedModels[0]">
                <img v-if="modelLogo(selectedModels[0].modelId, selectedModels[0].providerName, selectedModels[0].providerId)" :src="modelLogo(selectedModels[0].modelId, selectedModels[0].providerName, selectedModels[0].providerId)" alt="" />
                <span v-else class="model-fallback">{{ selectedModels[0].displayName.charAt(0).toUpperCase() }}</span>
              </template>
            </span>
            <span class="model-trigger-label">{{ selectedModelLabel() }}</span>
            <svg class="chevron" :class="{ open: showModelMenu }" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
              <path d="m6 9 6 6 6-6"/>
            </svg>
          </button>

          <div v-if="showModelMenu" class="model-menu">
            <div class="menu-title">选择回答模型</div>
            <div v-for="group in ai.groupedModels" :key="group.id" class="model-group">
              <div class="group-label">{{ group.name }}</div>
              <button
                v-for="model in group.models"
                :key="modelKey(model)"
                class="model-row"
                :class="{ active: selectedModelKeys.includes(modelKey(model)) }"
                @click="toggleModel(model)"
                @dblclick="selectOnly(model)"
              >
                <img v-if="modelLogo(model.modelId, model.providerName, model.providerId)" :src="modelLogo(model.modelId, model.providerName, model.providerId)" class="model-logo" alt="" />
                <span v-else class="model-logo fallback">{{ model.displayName.charAt(0).toUpperCase() }}</span>
                <span class="model-info">
                  <span class="model-name">{{ model.displayName }}</span>
                  <span class="model-meta">{{ capabilitiesLabel(model).join(' · ') || model.modelId }}</span>
                </span>
              </button>
            </div>
            <div class="menu-foot">点击切换模型。</div>
          </div>
        </div>

        <div
          ref="messagesEl"
          class="messages"
          :class="{ 'messages--no-floating-model': activeConversationIsMetadataExtraction }"
          @click="onMsgContainerClick"
        >
        <div v-if="!activeConversation?.nodes.length" class="empty-chat">
          <div class="empty-orb">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
              <path d="M12 3 13.8 8.2 19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/>
              <path d="M19 15v4"/><path d="M17 17h4"/>
            </svg>
          </div>
          <p>基于这篇论文开始新对话</p>
          <span>选择模型，开始与这篇论文对话。</span>
        </div>

        <template v-for="node in activeConversation?.nodes ?? []" :key="node.id">
          <div v-if="node.role === 'user'" class="user-row">
            <div class="user-msg-wrap">
              <!-- Edit mode -->
              <template v-if="editingNodeId === node.id">
                <textarea
                  class="user-edit-textarea"
                  v-model="editingText"
                  rows="1"
                  @keydown="onEditKeydown($event, node)"
                  @input="($event.target as HTMLTextAreaElement).style.height = 'auto'; ($event.target as HTMLTextAreaElement).style.height = ($event.target as HTMLTextAreaElement).scrollHeight + 'px'"
                  @vue:mounted="(el: any) => { el.style.height = el.scrollHeight + 'px'; el.focus(); el.setSelectionRange(el.value.length, el.value.length) }"
                />
                <div class="user-edit-actions">
                  <button class="edit-confirm-btn" :disabled="!editingText.trim()" @click="submitEdit(node)">
                    发送
                  </button>
                  <button class="edit-cancel-btn" @click="cancelEdit()">取消</button>
                </div>
              </template>
              <!-- Normal mode -->
              <template v-else>
                <!-- Context banner: shows what was ACTUALLY sent to the AI for this message -->
                <div v-if="hasContextBanner(node.id)" class="context-banner">
                  <button class="ctx-pills" @click="toggleContextPanel(node.id)" :title="expandedContextId === node.id ? '收起' : '查看发送给 AI 的上下文'">
                    <template v-if="getFirstAnswer(node.id)?.contextContent">
                      <span v-if="getFirstAnswer(node.id)!.contextContent!.metadata" class="ctx-pill ctx-meta">元数据</span>
                      <span v-if="getFirstAnswer(node.id)!.contextContent!.summary" class="ctx-pill ctx-summary">AI 总结</span>
                      <span v-if="getFirstAnswer(node.id)!.contextContent!.fulltext" class="ctx-pill ctx-fulltext">全文</span>
                      <span v-if="getFirstAnswer(node.id)!.usedPdf" class="ctx-pill ctx-pdf">PDF</span>
                    </template>
                    <template v-else>
                      <!-- contextContent not yet received or old conversation: fall back to contextMode -->
                      <span v-if="getFirstAnswer(node.id)?.contextMode === 'metadata'" class="ctx-pill ctx-meta">元数据</span>
                      <span v-if="getFirstAnswer(node.id)?.contextMode === 'summary' || getFirstAnswer(node.id)?.contextMode === 'summary+fulltext'" class="ctx-pill ctx-summary">AI 总结</span>
                      <span v-if="getFirstAnswer(node.id)?.contextMode === 'fulltext' || getFirstAnswer(node.id)?.contextMode === 'summary+fulltext'" class="ctx-pill ctx-fulltext">全文</span>
                      <span v-if="getFirstAnswer(node.id)?.usedPdf" class="ctx-pill ctx-pdf">PDF</span>
                    </template>
                    <svg class="ctx-chevron" :class="{ open: expandedContextId === node.id }" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <polyline points="6 9 12 15 18 9"/>
                    </svg>
                  </button>
                  <div v-if="expandedContextId === node.id" class="ctx-preview">
                    <template v-if="getFirstAnswer(node.id)?.contextContent">
                      <div v-if="getFirstAnswer(node.id)!.contextContent!.metadata" class="ctx-section">
                        <div class="ctx-section-label">元数据</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.metadata }}</pre>
                      </div>
                      <div v-if="getFirstAnswer(node.id)!.contextContent!.summary" class="ctx-section">
                        <div class="ctx-section-label">AI 总结</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.summary }}</pre>
                      </div>
                      <div v-if="getFirstAnswer(node.id)!.contextContent!.fulltext" class="ctx-section">
                        <div class="ctx-section-label">全文</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.fulltext }}</pre>
                      </div>
                      <div v-if="getFirstAnswer(node.id)!.usedPdf && !getFirstAnswer(node.id)!.contextContent!.fulltext" class="ctx-section">
                        <pre class="ctx-preview-text">PDF 文件已直接发送给模型</pre>
                      </div>
                    </template>
                    <div v-else class="ctx-loading">{{ getFirstAnswer(node.id)?.streaming ? '等待后端响应…' : '暂无上下文记录（旧对话不支持）' }}</div>
                  </div>
                </div>
                <div class="user-bubble">{{ node.content }}</div>
                <div class="msg-footer user-footer">
                  <div class="msg-actions">
                    <button class="action-btn" :class="{ done: copiedIds.has(node.id) }" title="复制" @click="copyText(node.id, node.content)">
                      <svg v-if="copiedIds.has(node.id)" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <polyline points="20 6 9 17 4 12"/>
                      </svg>
                      <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
                        <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
                      </svg>
                    </button>
                    <button class="action-btn" title="编辑" :disabled="hasStreaming" @click="startEdit(node)">
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                        <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
                      </svg>
                    </button>
                  </div>
                </div>
              </template>
            </div>
          </div>

          <div v-else class="answer-group">
            <div
              v-for="answer in activeAnswersForGroup(node)"
              :key="answer.id"
              class="answer-wrap"
              :class="{ streaming: answer.streaming, error: answer.error }"
            >
              <article
                class="answer-card"
                :class="{ streaming: answer.streaming, error: answer.error }"
              >
                <div class="answer-head">
                  <div class="answer-model">
                    <img
                      v-if="modelLogo(answer.modelId, answer.providerName, answer.providerId)"
                      :src="modelLogo(answer.modelId, answer.providerName, answer.providerId)"
                      class="answer-logo"
                      alt=""
                    />
                    <span v-else class="answer-logo fallback">{{ answer.modelName.charAt(0).toUpperCase() }}</span>
                    <span class="answer-name">{{ answerModelLabel(answer) }}</span>
                    <span
                      v-if="answer.usedPdf"
                      class="pdf-badge"
                      title="已将 PDF 直接发送给模型"
                    >PDF</span>
                    <span v-if="answer.streaming" class="live-dot" />
                  </div>
                </div>

                <!-- Thinking / reasoning content (collapsible) -->
                <details v-if="answer.reasoningContent" class="reasoning-section">
                  <summary class="reasoning-summary">
                    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="reasoning-chevron">
                      <path d="m9 18 6-6-6-6"/>
                    </svg>
                    思考过程
                    <span v-if="answer.streaming && !answer.content" class="reasoning-live-dot" />
                  </summary>
                  <pre class="reasoning-body">{{ answer.reasoningContent }}</pre>
                </details>

                <div
                  class="answer-body markdown-body"
                  :class="{ pending: answer.streaming && !answer.content && !answer.reasoningContent }"
                >
                  <template v-if="answer.streaming">
                    <div v-if="answer.content" v-html="renderMarkdown(answer.content)" />
                    <div v-else-if="!answer.reasoningContent" class="thinking-placeholder">{{ answer.withReasoning ? '正在思考…' : '生成中…' }}</div>
                  </template>
                  <template v-else>
                    <template v-for="(seg, si) in getSegments(answer.content)" :key="si">
                      <div v-if="seg.type === 'md'" v-html="seg.html" />
                      <MermaidBlock v-else :src="seg.src" />
                    </template>
                  </template>
                </div>
              </article>

              <div class="msg-footer answer-foot">
                <div class="msg-actions">
                  <button class="action-btn" :class="{ done: copiedIds.has(answer.id) }" title="复制回答" @click="copyText(answer.id, answer.content)">
                    <svg v-if="copiedIds.has(answer.id)" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <polyline points="20 6 9 17 4 12"/>
                    </svg>
                    <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
                      <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
                    </svg>
                  </button>
                  <button class="action-btn" :disabled="answer.streaming || hasStreaming" title="重新生成" @click="regenerate(node, answer)">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M21 12a9 9 0 1 1-2.64-6.36"/>
                      <path d="M21 3v6h-6"/>
                    </svg>
                  </button>
                </div>
                <div class="msg-usage">
                  <span v-if="!answer.streaming" class="usage-tokens">↓{{ answer.tokenEstimate ?? estimateTokens(answer.content) }}</span>
                  <span v-if="!answer.streaming && answerSpeed(answer)" class="msg-speed">{{ answerSpeed(answer) }}</span>
                  <span v-if="answer.error" class="error-badge">出错</span>
                </div>
              </div>

              <div v-if="node.answers.length > 1" class="variant-bar">
                <div
                  v-for="slot in node.answers"
                  :key="slot.id"
                  class="variant-slot"
                >
                  <button
                    class="variant-btn"
                    :class="{
                      active: activeAnswerForGroup(node)?.id === slot.id,
                      streaming: slot.streaming,
                      error: slot.error,
                    }"
                    :title="answerModelLabel(slot)"
                    @click="setActiveAnswer(node.id, slot.id)"
                  >
                    <img
                      v-if="modelLogo(slot.modelId, slot.providerName, slot.providerId)"
                      :src="modelLogo(slot.modelId, slot.providerName, slot.providerId)"
                      class="variant-logo"
                      alt=""
                    />
                    <span v-else class="variant-letter">{{ slot.modelName.charAt(0).toUpperCase() }}</span>
                  </button>
                </div>
              </div>
            </div>
          </div>
        </template>
        </div>

        <footer class="composer">
        <div class="context-bar">
          <button
            class="context-btn"
            :class="{ active: contextMode === 'metadata' }"
            @click="toggleContext('metadata')"
          >元数据</button>
          <button
            class="context-btn"
            :class="{ active: hasSummary }"
            :disabled="!summaryAvailable"
            :title="summaryAvailable ? 'AI 总结（可与全文同时选择）' : '尚无 AI 总结，请先生成'"
            @click="toggleContext('summary')"
          >AI 总结</button>
          <button
            class="context-btn"
            :class="{ active: hasFulltext }"
            :disabled="!fulltextReady"
            :title="fulltextReady ? '全文（可与 AI 总结同时选择）' : '请先获取全文'"
            @click="toggleContext('fulltext')"
          >全文</button>
          <button
            class="context-btn context-btn-pdf"
            :class="{ active: usePdf }"
            :disabled="!pdfSupported"
            :title="pdfSupported ? 'PDF（直接将 PDF 文件发给模型，仅 OpenRouter 支持）' : '当前模型不支持直接上传 PDF'"
            @click="usePdf = pdfSupported ? !usePdf : usePdf"
          >PDF</button>
          <span v-if="!fulltextReady && !fulltextChecking" class="context-hint">请先获取全文</span>
        </div>
        <div class="composer-box">
          <textarea
            ref="textareaEl"
            v-model="input"
            class="composer-input"
            rows="1"
            :disabled="hasStreaming"
            placeholder="问这篇论文里的任何问题…"
            @keydown="handleKeydown"
            @compositionstart="onCompositionStart"
            @compositionend="onCompositionEnd"
          />
          <div class="composer-toolbar">
            <button class="toolbar-btn" title="新建对话" @click="startNewConversation(true)">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                <path d="M18.375 2.625a2.121 2.121 0 1 1 3 3L12 15l-4 1 1-4Z"/>
              </svg>
            </button>

            <!-- Reasoning / thinking mode picker -->
            <div ref="reasoningRoot" class="reasoning-picker">
              <button
                class="toolbar-btn"
                :class="{ 'toolbar-btn-active': useReasoning }"
                title="思考模式"
                @click="reasoningOpen = !reasoningOpen"
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9">
                  <path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z"/>
                  <path d="M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z"/>
                  <path d="M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4"/>
                </svg>
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

            <span class="toolbar-spacer" />

            <button v-if="hasStreaming" class="send-btn stop-btn" title="停止生成" @click="stopAllStreaming">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor">
                <rect width="14" height="14" x="5" y="5" rx="2"/>
              </svg>
            </button>
            <button v-else class="send-btn" :class="{ active: canSend }" :disabled="!canSend" @click="sendMessage">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
                <path d="m22 2-7 20-4-9-9-4Z"/>
                <path d="M22 2 11 13"/>
              </svg>
            </button>
          </div>
        </div>
        </footer>

        <div v-if="!props.standalone && showHistory" class="history-layer" @mousedown.self="showHistory = false">
          <aside class="history-panel">
            <div class="history-head">
              <div>
                <h3>对话历史</h3>
                <p>{{ conversations.length }} 个历史对话</p>
              </div>
              <button class="icon-btn" @click="showHistory = false">
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M18 6 6 18"/><path d="m6 6 12 12"/>
                </svg>
              </button>
            </div>

            <button class="new-chat-btn" @click="startNewConversation(true)">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 5v14"/><path d="M5 12h14"/>
              </svg>
              新对话
            </button>

            <div class="history-list">
              <div
                v-for="conv in conversations"
                :key="conv.id"
                class="history-item"
                :class="{ active: conv.id === activeConversation?.id }"
                @click="openConversation(conv.id)"
              >
                <span class="history-title">{{ conv.title }}</span>
                <span class="history-meta">{{ conv.nodes.filter(n => n.role === 'user').length }} 问 · {{ formatTime(conv.updatedAt) }}</span>
                <button class="history-delete" title="删除" @click="deleteConversation(conv.id, $event)">
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/>
                  </svg>
                </button>
              </div>
              <div v-if="conversations.length === 0" class="history-empty">暂无历史对话</div>
            </div>
          </aside>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.paper-ai {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: var(--bg-primary);
}
.paper-ai.standalone {
  flex-direction: row;
  background: #ffffff;
}

.chat-main {
  position: relative;
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.history-sidebar {
  width: 236px;
  min-width: 196px;
  max-width: 280px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  min-height: 0;
  padding: 12px 8px;
  border-right: 1px solid rgba(0, 0, 0, 0.08);
  background: #f5f5f7;
}

.history-sidebar-head {
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 8px;
  color: #6e6e73;
  font-size: 12px;
  font-weight: 650;
}

.history-sidebar-head span:last-child {
  min-width: 20px;
  height: 20px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  color: #8a8a8e;
  background: rgba(0, 0, 0, 0.05);
  font-size: 11px;
}

.sidebar-new-chat {
  margin: 8px 4px 10px;
}

.sidebar-history-list {
  padding: 0 0 6px;
}

.center-hint {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 24px;
  text-align: center;
  color: var(--text-secondary);
}
.center-hint h3 { font-size: var(--font-size-md); font-weight: 650; color: var(--text-primary); }
.center-hint p { max-width: 230px; font-size: var(--font-size-sm); line-height: 1.5; }

.ai-header {
  height: 48px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 0 10px 0 12px;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-secondary) 68%, var(--bg-primary));
}
.standalone .ai-header {
  height: 46px;
  padding: 0 20px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.06);
  background: #ffffff;
}

.header-main {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 7px;
}
.standalone .header-main {
  padding-right: clamp(120px, 42vw, 280px);
}
.spark-icon {
  display: inline-flex;
  color: var(--accent);
  flex-shrink: 0;
}
.header-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-sm);
  font-weight: 650;
  color: var(--text-primary);
}
.standalone .header-title {
  font-size: 14px;
  color: #1c1c1e;
}
.header-actions {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}
.standalone .header-actions {
  margin-left: 12px;
}

.model-picker { position: relative; }
.floating-model-picker {
  position: absolute;
  top: 58px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 70;
}
.standalone .floating-model-picker {
  top: 6px;
  left: auto;
  right: 20px;
  transform: none;
  z-index: 80;
}
.standalone .model-trigger {
  height: 34px;
  width: min(220px, calc(100vw - 180px));
  border-radius: 10px;
  box-shadow: none;
  background: #ffffff;
}
.standalone .model-menu {
  left: auto;
  right: 0;
  transform: none;
  width: min(300px, calc(100vw - 40px));
}
.model-trigger {
  height: 36px;
  width: min(240px, calc(100vw - 40px));
  display: grid;
  grid-template-columns: 20px 1fr 16px;
  align-items: center;
  gap: 7px;
  padding: 0 11px;
  border: 1px solid var(--border-default);
  border-radius: 12px;
  color: var(--text-primary);
  background: color-mix(in srgb, var(--bg-primary) 90%, transparent);
  backdrop-filter: blur(18px) saturate(1.35);
  -webkit-backdrop-filter: blur(18px) saturate(1.35);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .12);
}
.model-trigger:hover { background: var(--bg-hover); }
.model-trigger-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.model-trigger-icon img {
  width: 18px;
  height: 18px;
  border-radius: 5px;
  object-fit: contain;
}
.model-fallback {
  width: 18px;
  height: 18px;
  border-radius: 5px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  color: var(--accent);
}
.model-trigger-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
  font-weight: 500;
  text-align: center;
}
.chevron { color: var(--text-tertiary); transition: transform .16s ease; flex-shrink: 0; }
.chevron.open { transform: rotate(180deg); }

.icon-btn {
  width: 30px;
  height: 30px;
  border-radius: 9px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
  border: 1px solid transparent;
}
.icon-btn:hover {
  color: var(--accent);
  border-color: var(--border-default);
  background: var(--bg-hover);
}

.model-menu {
  position: absolute;
  top: calc(100% + 8px);
  left: 50%;
  transform: translateX(-50%);
  z-index: 80;
  width: min(270px, calc(100vw - 32px));
  max-height: 420px;
  overflow-y: auto;
  padding: 8px;
  border-radius: 14px;
  border: 1px solid color-mix(in srgb, var(--border-default) 85%, transparent);
  background: color-mix(in srgb, var(--bg-primary) 94%, transparent);
  backdrop-filter: blur(18px) saturate(1.5);
  -webkit-backdrop-filter: blur(18px) saturate(1.5);
  box-shadow: 0 14px 40px rgba(0, 0, 0, .16);
}
.menu-title {
  padding: 4px 7px 8px;
  font-size: 12px;
  font-weight: 650;
  color: var(--text-primary);
}
.model-group + .model-group { margin-top: 8px; }
.group-label {
  padding: 4px 7px;
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: .04em;
}
.model-row {
  width: 100%;
  display: grid;
  grid-template-columns: 24px minmax(0, 1fr);
  align-items: center;
  gap: 7px;
  padding: 7px;
  border-radius: 10px;
  text-align: left;
}
.model-row:hover { background: var(--bg-hover); }
.model-row.active { background: color-mix(in srgb, var(--accent) 11%, transparent); }

.model-logo {
  width: 22px;
  height: 22px;
  border-radius: 6px;
  object-fit: contain;
}
.model-logo.fallback {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  background: var(--bg-secondary);
  font-weight: 700;
  font-size: 11px;
}
.model-info {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.model-name,
.model-meta {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.model-name {
  font-size: 12px;
  font-weight: 650;
  color: var(--text-primary);
}
.model-meta {
  font-size: 10px;
  color: var(--text-tertiary);
}
.menu-foot {
  padding: 8px 7px 3px;
  font-size: 10px;
  color: var(--text-tertiary);
}

.messages {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 58px 12px 12px;
}
.messages--no-floating-model {
  padding-top: 12px;
}
.standalone .messages {
  padding: 24px 20px 0;
}
.empty-chat {
  min-height: calc(100% - 44px);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  text-align: center;
  color: var(--text-secondary);
}
.standalone .empty-chat {
  min-height: 100%;
}
.empty-orb {
  width: 46px;
  height: 46px;
  border-radius: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 11%, transparent);
}
.empty-chat p {
  margin: 0;
  font-size: var(--font-size-sm);
  font-weight: 650;
  color: var(--text-primary);
}
.empty-chat span {
  max-width: 230px;
  font-size: 12px;
  line-height: 1.45;
  color: var(--text-tertiary);
}

.user-row {
  display: flex;
  justify-content: flex-end;
  margin: 8px 0 14px;
}
.user-msg-wrap {
  max-width: 88%;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
}
.user-bubble {
  padding: 8px 11px;
  border-radius: 14px 14px 4px 14px;
  color: #fff;
  background: var(--accent);
  font-size: 14px;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
  -webkit-user-select: text;
}

.user-edit-textarea {
  width: 100%;
  min-height: 38px;
  max-height: 220px;
  padding: 8px 11px;
  border-radius: 14px 14px 4px 14px;
  border: 2px solid var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary));
  color: var(--text-primary);
  font-size: 14px;
  line-height: 1.45;
  resize: none;
  overflow-y: auto;
  outline: none;
  font-family: inherit;
  box-sizing: border-box;
}
.user-edit-actions {
  display: flex;
  gap: 6px;
  margin-top: 5px;
  justify-content: flex-end;
}
.edit-confirm-btn {
  padding: 3px 12px;
  border-radius: 7px;
  border: none;
  background: var(--accent);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.edit-confirm-btn:disabled { opacity: .45; cursor: not-allowed; }
.edit-cancel-btn {
  padding: 3px 10px;
  border-radius: 7px;
  border: 1px solid var(--border-default);
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  cursor: pointer;
}
.edit-cancel-btn:hover { background: var(--bg-hover); }

.answer-group {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 0 0 15px;
}
.answer-wrap {
  display: flex;
  flex-direction: column;
}
.answer-card {
  border: 1px solid var(--border-subtle);
  border-radius: 13px;
  background: color-mix(in srgb, var(--bg-secondary) 42%, var(--bg-primary));
  overflow: hidden;
}
.answer-wrap:hover .answer-foot { opacity: 1; }
.answer-card.streaming {
  border-color: color-mix(in srgb, var(--accent) 34%, var(--border-subtle));
}
.answer-card.error {
  border-color: color-mix(in srgb, #ff3b30 40%, var(--border-subtle));
}
.answer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 9px;
  border-bottom: 1px solid var(--border-subtle);
}
.answer-model {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 7px;
}
.pdf-badge {
  display: inline-flex;
  align-items: center;
  padding: 1px 6px;
  border-radius: 4px;
  background: #fee2e2;
  color: #b91c1c;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.02em;
}
.answer-logo {
  width: 21px;
  height: 21px;
  border-radius: 6px;
  object-fit: contain;
  flex-shrink: 0;
}
.answer-logo.fallback {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  font-size: 11px;
  font-weight: 750;
}
.answer-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 650;
}
.live-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 40%, transparent);
  animation: live-pulse 1.15s infinite;
  flex-shrink: 0;
}
@keyframes live-pulse {
  70% { box-shadow: 0 0 0 7px color-mix(in srgb, var(--accent) 0%, transparent); }
  100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 0%, transparent); }
}
.mini-action {
  width: 25px;
  height: 25px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 8px;
  color: var(--text-tertiary);
  flex-shrink: 0;
}
.mini-action:hover:not(:disabled) {
  color: var(--accent);
  background: var(--bg-hover);
}
.mini-action:disabled { opacity: .45; cursor: not-allowed; }

.answer-body {
  padding: 10px 11px 8px;
  font-size: 14px;
  line-height: 1.58;
  color: var(--text-primary);
  word-break: break-word;
  user-select: text;
  -webkit-user-select: text;
}
.answer-body.pending {
  color: var(--text-tertiary);
  font-style: italic;
}
.answer-card.streaming .answer-body::after {
  content: '';
  display: inline-block;
  width: 6px;
  height: 1.1em;
  margin-left: 3px;
  vertical-align: -2px;
  background: var(--accent);
  animation: cursor-blink 1s steps(2, start) infinite;
}
@keyframes cursor-blink { 50% { opacity: 0; } }
.msg-footer {
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 4px;
  padding-left: 0;
  opacity: 0;
  transition: opacity .15s ease;
  justify-content: space-between;
}
.answer-foot {
  padding: 3px 7px 0;
}
.variant-bar {
  display: flex;
  align-items: center;
  gap: 4px;
  margin: 2px 7px 0;
  padding-top: 3px;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}
.variant-bar::-webkit-scrollbar {
  display: none;
}
.variant-slot {
  position: relative;
  display: inline-flex;
  flex: 0 0 auto;
}
.variant-btn {
  width: 26px;
  height: 26px;
  border-radius: 8px;
  border: 2px solid transparent;
  background: color-mix(in srgb, var(--text-primary) 4%, transparent);
  padding: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  overflow: hidden;
  transition: border-color .12s ease, background .12s ease, box-shadow .12s ease;
}
.variant-btn:hover {
  background: color-mix(in srgb, var(--text-primary) 8%, transparent);
}
.variant-btn.active {
  border-color: color-mix(in srgb, var(--accent) 72%, transparent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.variant-btn.error {
  border-color: color-mix(in srgb, #ff3b30 55%, transparent);
}
.variant-btn.streaming {
  animation: variant-pulse 1.2s ease-out infinite;
}
@keyframes variant-pulse {
  0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 34%, transparent); }
  70% { box-shadow: 0 0 0 4px color-mix(in srgb, var(--accent) 0%, transparent); }
  100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 0%, transparent); }
}
.variant-logo {
  width: 20px;
  height: 20px;
  object-fit: contain;
}
.variant-letter {
  color: var(--text-tertiary);
  font-size: 11px;
  font-weight: 750;
}
.user-footer {
  justify-content: flex-end;
  padding-right: 2px;
}
.user-row:hover .user-footer { opacity: 1; }
.answer-wrap.streaming .answer-foot,
.answer-wrap.error .answer-foot { opacity: 1; }

/* Message action buttons (copy / regen at bottom of messages) */
.msg-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}
.action-btn {
  width: 24px;
  height: 24px;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background .1s ease, color .1s ease;
  flex-shrink: 0;
}
.action-btn:hover:not(:disabled) {
  color: var(--text-primary);
  background: var(--bg-hover);
}
.action-btn:disabled { opacity: .4; cursor: not-allowed; }
.action-btn.done { color: #22c55e; }
.msg-usage {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  margin-left: auto;
  padding-left: 8px;
  justify-content: flex-end;
  flex: 1;
  font-size: 10.5px;
  color: var(--text-tertiary);
}
.usage-tokens { color: var(--text-tertiary); }
.msg-speed { color: color-mix(in srgb, var(--accent) 74%, var(--text-tertiary)); }
.error-badge { color: #ef4444; }

.composer {
  flex-shrink: 0;
  padding: 6px 10px 10px;
  border-top: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 94%, transparent);
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.standalone .composer {
  padding: 12px 20px 16px;
  border-top: none;
  background: #ffffff;
}
.standalone .composer-box {
  padding: 12px 12px 8px 14px;
  background: #f5f5f7;
  border-color: rgba(0, 0, 0, 0.09);
}
.standalone .composer-box:focus-within {
  border-color: rgba(34, 63, 121, 0.35);
  background: #fafafa;
}
.standalone .composer-input {
  min-height: 24px;
  max-height: 180px;
  padding: 0;
  font-size: 14px;
  line-height: 1.55;
}
.standalone .composer-toolbar {
  margin-top: 8px;
  padding: 0;
}
.standalone .toolbar-btn {
  width: 28px;
  height: 28px;
  border-radius: 7px;
}
.standalone .send-btn.active {
  background: #22c55e;
}
.standalone .send-btn.active:hover {
  background: #16a34a;
}
.composer-box {
  display: flex;
  flex-direction: column;
  border: 1.5px solid var(--border-default);
  border-radius: 13px;
  background: var(--bg-primary);
  transition: box-shadow .15s ease, border-color .15s ease;
}
.composer-box:focus-within {
  border-color: color-mix(in srgb, var(--accent) 55%, var(--border-default));
}
.context-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 2px;
}
.context-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 3px 10px;
  border-radius: 6px;
  font-size: 11.5px;
  font-weight: 500;
  color: var(--text-secondary);
  border: 1.5px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 70%, transparent);
  transition: all .12s ease;
}
.context-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}
.context-btn:disabled {
  color: var(--text-tertiary);
  border-color: var(--border-subtle);
  background: color-mix(in srgb, var(--text-primary) 4%, transparent);
  opacity: .48;
  cursor: not-allowed;
}
.context-btn:disabled:hover {
  color: var(--text-tertiary);
  background: color-mix(in srgb, var(--text-primary) 4%, transparent);
}
.context-btn.active {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 32%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 10%, transparent);
}
.context-hint {
  margin-left: 2px;
  color: var(--text-tertiary);
  font-size: 11.5px;
  line-height: 1;
}
.context-btn-pdf.active {
  color: #b91c1c;
  border-color: color-mix(in srgb, #b91c1c 32%, var(--border-default));
  background: color-mix(in srgb, #b91c1c 10%, transparent);
}

.composer-input {
  flex: 1;
  min-height: 68px;
  resize: none;
  padding: 10px 12px 0;
  border: none;
  outline: none;
  color: var(--text-primary);
  background: transparent;
  font: inherit;
  font-size: 14px;
  line-height: 1.45;
}
.composer-input:disabled { opacity: .65; }
.composer-toolbar {
  display: flex;
  align-items: center;
  gap: 1px;
  padding: 5px 7px 7px;
}
.toolbar-spacer { flex: 1; }
.toolbar-btn {
  position: relative;
  width: 30px;
  height: 30px;
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
  top: 2px;
  right: 2px;
  font-size: 8px;
  font-weight: 700;
  line-height: 1.4;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 15%, var(--bg-primary));
  border-radius: 3px;
  padding: 0 2px;
}
.send-btn {
  width: 30px;
  height: 30px;
  border: none;
  border-radius: 8px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: color-mix(in srgb, var(--text-primary) 24%, transparent);
  background: color-mix(in srgb, var(--text-primary) 7%, transparent);
  flex-shrink: 0;
  cursor: not-allowed;
  transition: background .15s ease, color .15s ease;
}
.send-btn.active {
  color: #fff;
  background: var(--accent);
  cursor: pointer;
}
.send-btn.active:hover { background: var(--accent-hover); }
.stop-btn {
  color: var(--text-primary);
  background: color-mix(in srgb, var(--text-primary) 9%, transparent);
  cursor: pointer;
}
.stop-btn:hover { background: color-mix(in srgb, var(--text-primary) 15%, transparent) !important; }

/* Reasoning popover */
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
  box-shadow: 0 10px 32px rgba(0,0,0,.18);
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
  border-radius: 10px;
  background: var(--border-default);
  transition: background .2s ease;
  flex-shrink: 0;
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
  box-shadow: 0 1px 3px rgba(0,0,0,.2);
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

/* Reasoning section in answer card */
.reasoning-section {
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--accent) 4%, transparent);
}
.reasoning-summary {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 6px 11px;
  font-size: 11px;
  font-weight: 650;
  color: var(--text-tertiary);
  cursor: pointer;
  list-style: none;
  user-select: none;
}
.reasoning-summary::-webkit-details-marker { display: none; }
.reasoning-section[open] .reasoning-chevron { transform: rotate(90deg); }
.reasoning-chevron { transition: transform .15s ease; flex-shrink: 0; }
.reasoning-live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
  margin-left: 4px;
  animation: live-pulse 1.15s infinite;
}
.reasoning-body {
  margin: 0;
  padding: 6px 11px 10px;
  font-size: 11px;
  line-height: 1.55;
  color: var(--text-secondary);
  font-family: var(--font-mono, ui-monospace, monospace);
  white-space: pre-wrap;
  word-break: break-word;
}
.thinking-placeholder {
  color: var(--text-tertiary);
  font-style: italic;
  font-size: var(--font-size-sm);
}

.history-layer {
  position: absolute;
  inset: 0;
  z-index: 100;
  background: rgba(0, 0, 0, .08);
}
.history-panel {
  position: absolute;
  top: 8px;
  right: 8px;
  bottom: 8px;
  width: min(288px, calc(100% - 16px));
  display: flex;
  flex-direction: column;
  border-radius: 16px;
  border: 1px solid color-mix(in srgb, var(--border-default) 84%, transparent);
  background: color-mix(in srgb, var(--bg-primary) 96%, transparent);
  backdrop-filter: blur(18px) saturate(1.45);
  -webkit-backdrop-filter: blur(18px) saturate(1.45);
  box-shadow: 0 18px 48px rgba(0,0,0,.18);
  overflow: hidden;
}
.history-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 13px 13px 10px;
  border-bottom: 1px solid var(--border-subtle);
}
.history-head h3 {
  margin: 0;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}
.history-head p {
  margin: 3px 0 0;
  font-size: 11px;
  color: var(--text-tertiary);
}
.new-chat-btn {
  margin: 10px 12px;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border-radius: 10px;
  color: var(--accent);
  border: 1px solid color-mix(in srgb, var(--accent) 38%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 7%, transparent);
  font-size: var(--font-size-sm);
  font-weight: 650;
}
.new-chat-btn:hover { background: color-mix(in srgb, var(--accent) 12%, transparent); }
.history-list {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  padding: 0 8px 10px;
}
.history-item {
  position: relative;
  width: 100%;
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 9px 32px 9px 10px;
  border-radius: 10px;
  text-align: left;
  cursor: pointer;
}
.history-item:hover { background: var(--bg-hover); }
.history-item.active { background: color-mix(in srgb, var(--accent) 11%, transparent); }
.history-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 650;
  color: var(--text-primary);
}
.history-meta {
  font-size: 10px;
  color: var(--text-tertiary);
}
.history-delete {
  position: absolute;
  top: 50%;
  right: 8px;
  width: 22px;
  height: 22px;
  transform: translateY(-50%);
  border-radius: 7px;
  color: var(--text-tertiary);
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
}
.history-item:hover .history-delete { opacity: 1; }
.history-delete:hover { color: #ff3b30; background: color-mix(in srgb, #ff3b30 9%, transparent); }
.history-empty {
  padding: 28px 8px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: 12px;
}

.primary-btn {
  padding: 7px 13px;
  border-radius: 10px;
  color: #fff;
  background: var(--accent);
  font-size: var(--font-size-xs);
  font-weight: 650;
}

/* ── Context banner ── */
.context-banner {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  margin-bottom: 5px;
  width: 100%;
}
.ctx-pills {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 7px 3px 6px;
  border-radius: 8px;
  border: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-secondary) 70%, transparent);
  cursor: pointer;
  font-size: 11px;
  color: var(--text-secondary);
  transition: background 0.14s;
  line-height: 1;
}
.ctx-pills:hover { background: var(--bg-hover); }
.ctx-pill {
  display: inline-flex;
  align-items: center;
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.02em;
}
.ctx-meta    { background: color-mix(in srgb, #6b7280 15%, transparent); color: #4b5563; }
.ctx-summary { background: color-mix(in srgb, #7c3aed 14%, transparent); color: #6d28d9; }
.ctx-fulltext{ background: color-mix(in srgb, #059669 14%, transparent); color: #047857; }
.ctx-pdf     { background: #fee2e2; color: #b91c1c; }
.ctx-chevron {
  color: var(--text-tertiary);
  transition: transform 0.16s ease;
  flex-shrink: 0;
}
.ctx-chevron.open { transform: rotate(180deg); }

.ctx-preview {
  margin-top: 4px;
  width: 100%;
  max-width: 340px;
  border: 1px solid var(--border-subtle);
  border-radius: 10px;
  background: var(--bg-primary);
  overflow: hidden;
}
.ctx-loading {
  padding: 10px 12px;
  font-size: 12px;
  color: var(--text-tertiary);
}
.ctx-section + .ctx-section {
  border-top: 1px solid var(--border-subtle);
}
.ctx-section-label {
  padding: 6px 12px 2px;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.04em;
  color: var(--text-tertiary);
  text-transform: uppercase;
}
.ctx-preview-text {
  margin: 0;
  padding: 10px 12px;
  font-size: 11.5px;
  line-height: 1.55;
  color: var(--text-secondary);
  white-space: pre-wrap;
  word-break: break-word;
  font-family: inherit;
  max-height: 260px;
  overflow-y: auto;
}
</style>
