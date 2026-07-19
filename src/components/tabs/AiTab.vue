<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useAiStore, type ModelOption } from '../../stores/ai'
import { useSettingsStore } from '../../stores/settings'
import MermaidBlock from '../MermaidBlock.vue'
import { renderMarkdown, getSegments } from '../../utils/renderMarkdown'
import { svgStringToPngBlob } from '../../utils/svgToPng'
import { copyPngBlobToClipboard } from '../../utils/clipboard'
import type { ChatContentPart, ChatMessage, PaperMeta, PaperStatus, PaperSection } from '../../types'
import { askAiText } from '../../stores/translationHistory'

const props = withDefaults(defineProps<{ slug: string | null; standalone?: boolean }>(), {
  standalone: false,
})
const emit = defineEmits<{ 'open-settings': [] }>()

const { t } = useI18n()
const ai = useAiStore()
const settingsStore = useSettingsStore()

interface Attachment {
  id: string
  type: 'image' | 'pdf'
  name: string
  dataUrl: string
}

type ChatNode =
  | { id: string; role: 'user'; content: string; attachments?: Attachment[]; createdAt: string }
  | { id: string; role: 'assistantGroup'; promptId: string; answers: AssistantAnswer[]; createdAt: string }
type AssistantGroupNode = Extract<ChatNode, { role: 'assistantGroup' }>

interface AssistantAnswer {
  id: string
  providerId: string
  providerName: string
  modelId: string
  modelName: string
  content: string
  // Throttled copy of `content` used for live markdown rendering while
  // streaming (re-rendering the full markdown on every token freezes the UI).
  displayContent?: string
  reasoningContent?: string
  withReasoning?: boolean
  createdAt: string
  startedAt?: number
  endedAt?: number
  streaming?: boolean
  error?: boolean
  errorText?: string
  tokenEstimate?: number
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  cacheHitTokens?: number
  costUsd?: number | null
  contextMode?: string
  usedPdf?: boolean
  // Titles of the paper sections selected as context for this turn.
  sectionTitles?: string[]
  source?: 'chat' | 'metadataExtraction'
  // Actual content injected into the system prompt, received via -context event
  contextContent?: { metadata: string; summary: string; fulltext: string; sections?: string }
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

interface StreamUsagePayload {
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cost_usd?: number | null
  cache_hit_tokens?: number
}

interface ExtractionProgressPayload {
  slug?: string
  ok?: boolean
}

type PaperContextMode = 'none' | 'metadata' | 'summary' | 'fulltext' | 'summary+fulltext'
type PaperContextSection = 'metadata' | 'summary' | 'fulltext'

const STORAGE_PREFIX = 'argus.paper-ai-conversations.v2'

const allSelectableModels = computed<ModelOption[]>(() =>
  ai.chatModels
)

const conversations = ref<Conversation[]>([])
const activeConversation = ref<Conversation | null>(null)
const input = ref('')
const attachments = ref<Attachment[]>([])
const fileInputRef = ref<HTMLInputElement | null>(null)
const messagesEl = ref<HTMLElement | null>(null)
const textareaEl = ref<HTMLTextAreaElement | null>(null)
const selectedModelKeys = ref<string[]>([])
const showModelMenu = ref(false)
const showHistory = ref(false)
const previewImage = ref<string | null>(null)
const previewPdf = ref<string | null>(null)
const modelMenuRoot = ref<HTMLElement | null>(null)
const unlisteners = new Map<string, UnlistenFn>()
// Maps answer.id -> backend request_id, so stopAllStreaming can tell the backend
// to truly cancel the in-flight HTTP request (stop the provider generating/billing).
const activeRequestIds = new Map<string, string>()
const fulltextReady = ref(false)
const fulltextChecking = ref(false)
const abstractAvailable = ref(false)
const activeAnswerTabs = ref<Record<string, string>>({})
let unlistenExtractionProgress: UnlistenFn | null = null
let unlistenMetaStart: UnlistenFn | null = null
let unlistenMetaDone: UnlistenFn | null = null
let unlistenMetaError: UnlistenFn | null = null

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
const sectionMenuRoot = ref<HTMLElement | null>(null)

// Context mode: how much paper content to inject as system prompt context
// Possible values: 'none' | 'metadata' | 'summary' | 'fulltext' | 'summary+fulltext'
// All options are independent toggles; none = no context injected.
const contextMode = ref<PaperContextMode>('none')
const usePdf = ref(false)
const summaryAvailable = ref(false)

// Chapter (section) context: multi-select of the paper's detected sections.
const availableSections = ref<PaperSection[]>([])
const selectedSectionTitles = ref<string[]>([])
const showSectionMenu = ref(false)
const sectionsActive = computed(() => selectedSectionTitles.value.length > 0)

async function loadSections(slug: string | null) {
  availableSections.value = []
  selectedSectionTitles.value = []
  showSectionMenu.value = false
  if (!slug) return
  try {
    const data = await invoke<{ sections: PaperSection[] } | null>('get_sections', { slug })
    availableSections.value = data?.sections ?? []
  } catch {
    availableSections.value = []
  }
}

function toggleSectionMenu() {
  if (!availableSections.value.length) return
  showSectionMenu.value = !showSectionMenu.value
}

// A section plus its descendants: the consecutive following entries whose level
// is deeper than its own. Selecting a parent recursively (de)selects children.
function sectionWithDescendants(i: number): string[] {
  const secs = availableSections.value
  const level = secs[i].level
  const titles = [secs[i].title]
  for (let j = i + 1; j < secs.length; j++) {
    if (secs[j].level > level) titles.push(secs[j].title)
    else break
  }
  return titles
}

function toggleSection(i: number) {
  const titles = sectionWithDescendants(i)
  const isSelected = selectedSectionTitles.value.includes(availableSections.value[i].title)
  if (isSelected) {
    selectedSectionTitles.value = selectedSectionTitles.value.filter(t => !titles.includes(t))
  } else {
    const set = new Set(selectedSectionTitles.value)
    titles.forEach(t => set.add(t))
    selectedSectionTitles.value = [...set]
  }
}

function selectAllSections() {
  selectedSectionTitles.value = availableSections.value.map(s => s.title)
}

function clearSelectedSections() {
  selectedSectionTitles.value = []
}

function onSectionsUpdatedEvent(e: Event) {
  const slug = (e as CustomEvent<{ slug?: string }>).detail?.slug
  if (slug && slug === props.slug) loadSections(slug)
}

// PDF mode uses OpenAI-compatible inline file content parts, which only
// OpenRouter reliably supports. Kimi / Moonshot endpoints reject the "file"
// part type, so we keep the toggle disabled for those providers.
// Ebook papers have no PDF at all — the toggle is disabled and the fulltext
// context modes carry the book content instead.
const paperIsEbook = ref(false)
const pdfSupported = computed(() =>
  !paperIsEbook.value &&
  selectedModels.value.some(m => {
    const p = ai.settings.providers.find(p => p.id === m.providerId)
    if (!p) return false
    return p.kind === 'openrouter' || p.base_url.toLowerCase().includes('openrouter')
  })
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

// The model is stateless: anything it should keep "seeing" (the full text,
// summary, selected chapters…) must be resent on EVERY turn. So each turn sends
// the currently-selected context in full. We deliberately do NOT strip context
// that already appeared in an earlier turn — that older behaviour made the model
// lose the paper after the first message. "Not sending it twice" is already
// guaranteed within a single request (one context block per turn).
function contextPlanForConversation() {
  return {
    contextMode: effectiveContextMode.value,
    usePdf: usePdf.value && pdfSupported.value,
    sectionTitles: [...selectedSectionTitles.value],
  }
}

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

function chatContentText(content: string | ChatContentPart[]): string {
  if (typeof content === 'string') return content
  return content
    .filter((p): p is Extract<ChatContentPart, { type: 'text' }> => p.type === 'text')
    .map(p => p.text)
    .join('\n')
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
    for (const msg of history.filter(m => {
      if (m.role === 'system') return false
      return chatContentText(m.content).trim().length > 0
    })) {
      const text = chatContentText(msg.content)
      if (msg.role === 'user') {
        nodes.push({ id: newId('user'), role: 'user', content: text, createdAt: nowIso() })
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
            content: text,
            createdAt: nowIso(),
            tokenEstimate: estimateTokens(text),
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
  if (!slug) { abstractAvailable.value = false; paperIsEbook.value = false; return }
  try {
    const meta = await invoke<PaperMeta>('get_paper_meta', { slug })
    if (props.slug === slug) {
      abstractAvailable.value = !!meta.abstract?.trim()
      paperIsEbook.value = !!meta.file_type && meta.file_type !== 'pdf'
    }
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

// ── Context badge selection persistence ─────────────────────────────────────
// The badge selection (context mode / PDF / sections) is persisted per paper in
// localStorage, which is shared across webviews. This keeps the sidebar tab and
// the standalone popup in sync, and lets us restore the selection when a
// conversation from history is reopened.
const CONTEXT_STORAGE_PREFIX = 'argus:ai-context'
function contextStorageKey(slug: string) {
  return `${CONTEXT_STORAGE_PREFIX}:${slug}`
}

interface ContextSelection {
  contextMode?: string
  usePdf?: boolean
  sectionTitles?: string[]
}

// Guards programmatic selection changes (paper switch, history open, cross-window
// sync) so they aren't re-persisted and echoed back into an update loop.
let restoringContext = false

function readContextSelection(slug: string): ContextSelection | null {
  try {
    const raw = localStorage.getItem(contextStorageKey(slug))
    return raw ? (JSON.parse(raw) as ContextSelection) : null
  } catch {
    return null
  }
}

function persistContextSelection() {
  if (!props.slug) return
  try {
    const sel: ContextSelection = {
      contextMode: contextMode.value,
      usePdf: usePdf.value,
      sectionTitles: selectedSectionTitles.value,
    }
    localStorage.setItem(contextStorageKey(props.slug), JSON.stringify(sel))
  } catch {
    // storage full / disabled — non-fatal
  }
}

// Apply a saved/derived badge selection, downgrading to what this paper actually
// supports (e.g. can't select fulltext before it's extracted, or summary before
// it's generated).
function applyContextSelection(sel: ContextSelection | null) {
  if (!sel) return
  const mode = (sel.contextMode as PaperContextMode) || 'none'
  const wantsSummary =
    (mode === 'summary' || mode === 'summary+fulltext') && summaryAvailable.value
  const wantsFulltext =
    (mode === 'fulltext' || mode === 'summary+fulltext') && fulltextReady.value
  if (wantsFulltext && wantsSummary) contextMode.value = 'summary+fulltext'
  else if (wantsFulltext) contextMode.value = 'fulltext'
  else if (wantsSummary) contextMode.value = 'summary'
  else contextMode.value = mode === 'metadata' ? 'metadata' : 'none'
  usePdf.value = !!sel.usePdf && pdfSupported.value
  selectedSectionTitles.value = Array.isArray(sel.sectionTitles) ? [...sel.sectionTitles] : []
}

// Restore the badges that were actually used in a saved conversation (from its
// most recent assistant turn) so reopening history reflects that turn's context.
function restoreContextFromConversation(conv: Conversation) {
  const nodes = conv.nodes ?? []
  for (let i = nodes.length - 1; i >= 0; i--) {
    const node = nodes[i]
    if (node.role === 'assistantGroup' && node.answers.length > 0) {
      const a = node.answers[0]
      const prev = restoringContext
      restoringContext = true
      try {
        applyContextSelection({
          contextMode: a.contextMode,
          usePdf: a.usedPdf,
          sectionTitles: a.sectionTitles,
        })
      } finally {
        restoringContext = prev
      }
      return
    }
  }
}

// Live cross-window sync: when the other webview persists a new selection for the
// current paper, adopt it here too.
function onContextStorageSync(e: StorageEvent) {
  if (!props.slug || e.key !== contextStorageKey(props.slug) || e.newValue == null) return
  const prev = restoringContext
  restoringContext = true
  try {
    applyContextSelection(JSON.parse(e.newValue) as ContextSelection)
  } catch {
    // ignore malformed payloads
  } finally {
    restoringContext = prev
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
  // Restore the badge selection this conversation was last used with.
  restoreContextFromConversation(conv)
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

// Persist after an in-place edit to the active conversation's nodes. If the
// conversation is now empty, drop it from history and reset to a blank one.
function afterConversationMutation() {
  const conv = activeConversation.value
  if (!conv) return
  if (conv.nodes.length === 0) {
    conversations.value = conversations.value.filter(c => c.id !== conv.id)
    if (props.slug) saveConversationsToPaper(props.slug).catch(() => {})
    startNewConversation(false)
  } else {
    persistActiveConversation()
  }
}

// Delete a whole exchange: the user message and its answer group (answers
// reference the prompt via promptId, so they can't stand on their own).
function deleteExchange(userNodeId: string) {
  const conv = activeConversation.value
  if (!conv || hasStreaming.value) return
  const removedGroupIds = conv.nodes
    .filter(n => n.role === 'assistantGroup' && n.promptId === userNodeId)
    .map(n => n.id)
  conv.nodes = conv.nodes.filter(
    n => n.id !== userNodeId && !(n.role === 'assistantGroup' && n.promptId === userNodeId),
  )
  if (removedGroupIds.length) {
    const next = { ...activeAnswerTabs.value }
    removedGroupIds.forEach(id => delete next[id])
    activeAnswerTabs.value = next
  }
  afterConversationMutation()
}

// Delete a single AI answer. With multiple model variants, remove just that
// variant; if it is the only answer, remove the whole exchange.
function deleteAnswer(group: ChatNode, answer: AssistantAnswer) {
  if (group.role !== 'assistantGroup' || hasStreaming.value || answer.streaming) return
  if (group.answers.length > 1) {
    group.answers = group.answers.filter(a => a.id !== answer.id)
    if (activeAnswerTabs.value[group.id] === answer.id && group.answers[0]) {
      setActiveAnswer(group.id, group.answers[0].id)
    }
    afterConversationMutation()
  } else {
    deleteExchange(group.promptId)
  }
}

function firstUserTitle(nodes: ChatNode[]) {
  const first = nodes.find((n): n is Extract<ChatNode, { role: 'user' }> => n.role === 'user')
  if (!first) return ''
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
  if (haystack.includes('gemma')) return modelIconMap.gemma
  if (haystack.includes('gemini') || haystack.includes('google')) return modelIconMap.gemini
  if (haystack.includes('qwen') || haystack.includes('通义') || haystack.includes('alibaba')) return modelIconMap.qwen ?? modelIconMap.alibaba
  if (haystack.includes('kimi') || haystack.includes('moonshot')) return modelIconMap.kimi
  if (haystack.includes('grok') || haystack.includes('xai')) return modelIconMap.grok ?? modelIconMap.xai
  if (haystack.includes('zhipu') || haystack.includes('智谱') || haystack.includes('glm')) return modelIconMap.zhipu
  if (haystack.includes('baidu') || haystack.includes('ernie')) return modelIconMap.baidu
  if (haystack.includes('doubao') || haystack.includes('bytedance')) return modelIconMap.bytedance
  if (haystack.includes('mistral') || haystack.includes('huggingface')) return modelIconMap.huggingface
  if (haystack.includes('gpt') || haystack.includes('openai')) return modelIconMap.openai
  // Ollama is a host, not a model brand — the provider name pollutes the
  // haystack, so match its mark only after every real model brand above.
  if (haystack.includes('ollama')) return modelIconMap['ollama-color']
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

function buildHistoryUntil(conv: Conversation, stopGroupId?: string): ChatMessage[] {
  const messages: ChatMessage[] = []
  for (const node of conv.nodes) {
    if (node.role === 'assistantGroup') {
      if (node.id === stopGroupId) break
      // Use the answer the user is actually viewing for this group; only fall
      // back to "first non-empty" if that branch is empty/errored. The old
      // "first non-empty" logic could pick a different model's (shorter) answer
      // in multi-model turns, dropping the real reply from the history.
      const active = activeAnswerForGroup(node)
      const answer =
        active && !active.error && active.content.trim()
          ? active
          : node.answers.find(a => !a.error && a.content.trim()) ??
            node.answers.find(a => a.content.trim())
      if (answer && answer.content.trim()) {
        messages.push({ role: 'assistant', content: answer.content })
      }
    } else if (node.role === 'user') {
      if (node.attachments?.length) {
        messages.push({ role: 'user', content: buildUserContentParts(node.content, node.attachments) })
      } else {
        messages.push({ role: 'user', content: node.content })
      }
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
  const contextPlan = contextPlanForConversation()

  const userNode: ChatNode = {
    id: newId('user'),
    role: 'user',
    content: text,
    attachments: attachments.value.length > 0 ? [...attachments.value] : undefined,
    createdAt: nowIso(),
  }
  const group: ChatNode = {
    id: newId('group'),
    role: 'assistantGroup',
    promptId: userNode.id,
    createdAt: nowIso(),
    answers: selectedModels.value.map(model => modelToAnswer(model, contextPlan.contextMode, contextPlan.usePdf, contextPlan.sectionTitles)),
  }
  if (group.answers[0]) setActiveAnswer(group.id, group.answers[0].id)
  conv.nodes.push(userNode, group)
  conv.title = firstUserTitle(conv.nodes) || conv.title
  input.value = ''
  attachments.value = []
  persistActiveConversation()
  await nextTick()
  resizeTextarea()
  scrollToBottom(true)

  const history = buildHistoryUntil(conv, group.id)
  await Promise.all(group.answers.map(answer => streamAnswer(conv, answer, history)))
}

function modelToAnswer(
  model: ModelOption,
  contextModeToSend: PaperContextMode,
  usePdfToSend: boolean,
  sectionTitlesToSend: string[],
): AssistantAnswer {
  return {
    id: newId('answer'),
    providerId: model.providerId,
    providerName: model.providerName,
    modelId: model.modelId,
    modelName: model.displayName,
    content: '',
    withReasoning: useReasoning.value,
    createdAt: nowIso(),
    contextMode: contextModeToSend,
    usedPdf: usePdfToSend,
    sectionTitles: sectionTitlesToSend,
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
    ra.tokenEstimate = undefined
    ra.inputTokens = undefined
    ra.outputTokens = undefined
    ra.totalTokens = undefined
    ra.costUsd = undefined
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
  const contextPlan = contextPlanForConversation()

  const group: ChatNode = {
    id: newId('group'),
    role: 'assistantGroup',
    promptId: node.id,
    createdAt: nowIso(),
    answers: selectedModels.value.map(model => modelToAnswer(model, contextPlan.contextMode, contextPlan.usePdf, contextPlan.sectionTitles)),
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
  // Tell the backend to truly cancel each in-flight request (closes the HTTP
  // stream so the provider stops generating / billing).
  for (const requestId of [...activeRequestIds.values()]) {
    invoke('cancel_ai_request', { requestId }).catch(() => {})
  }
  activeRequestIds.clear()
  for (const [key, off] of unlisteners.entries()) {
    off()
    unlisteners.delete(key)
    const answerId = key.replace(/-(reasoning|context|usage)$/, '')
    const ra = findReactiveAnswer(answerId)
    if (ra?.streaming) {
      ra.streaming = false
      ra.endedAt = performance.now()
      flushStreamRender(ra)
    }
  }
  persistActiveConversation()
}

// ── Throttled streaming render ────────────────────────────────────────────────
// Re-rendering the full markdown (markdown-it + KaTeX + highlight.js) on every
// streamed token is O(n²) and freezes the UI on long answers. We instead refresh
// a `displayContent` copy at most once per STREAM_RENDER_MS.
const STREAM_RENDER_MS = 90
const streamRenderTimers = new Map<string, ReturnType<typeof setTimeout>>()
const streamRenderLast = new Map<string, number>()

function scheduleStreamRender(ans: AssistantAnswer) {
  const now = Date.now()
  const last = streamRenderLast.get(ans.id) ?? 0
  const elapsed = now - last
  if (elapsed >= STREAM_RENDER_MS) {
    streamRenderLast.set(ans.id, now)
    ans.displayContent = ans.content
    scrollToBottom()
    return
  }
  if (streamRenderTimers.has(ans.id)) return
  const timer = setTimeout(() => {
    streamRenderTimers.delete(ans.id)
    streamRenderLast.set(ans.id, Date.now())
    const live = findReactiveAnswer(ans.id)
    if (live) { live.displayContent = live.content; scrollToBottom() }
  }, STREAM_RENDER_MS - elapsed)
  streamRenderTimers.set(ans.id, timer)
}

// Final flush so the last tokens are shown even if a throttle window was pending.
function flushStreamRender(ans: AssistantAnswer) {
  const timer = streamRenderTimers.get(ans.id)
  if (timer) { clearTimeout(timer); streamRenderTimers.delete(ans.id) }
  streamRenderLast.delete(ans.id)
  ans.displayContent = ans.content
}

// Clear every pending throttle timer (used on session switch / unmount so
// scheduled renders don't fire against a stale/torn-down conversation).
function clearAllStreamRenderTimers() {
  for (const timer of streamRenderTimers.values()) clearTimeout(timer)
  streamRenderTimers.clear()
  streamRenderLast.clear()
}

async function streamAnswer(conv: Conversation, answer: AssistantAnswer, history: ChatMessage[]) {
  if (!props.slug) return
  const eventName = `paper-ai-chat-${answer.id}`
  const reasoningEventName = `${eventName}-reasoning`
  // Backend cancellation id: sent to `chat_with_paper_event`, used by
  // stopAllStreaming to invoke `cancel_ai_request`.
  const requestId = crypto.randomUUID()
  activeRequestIds.set(answer.id, requestId)

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
    ra.tokenEstimate = undefined
    ra.inputTokens = undefined
    ra.outputTokens = undefined
    ra.totalTokens = undefined
    ra.costUsd = undefined
  }

  const unlisten = await listen<StreamPayload>(eventName, (event) => {
    if (event.payload.done) return
    const reactiveAns = findReactiveAnswer(answer.id)
    if (!reactiveAns) return
    reactiveAns.content += event.payload.delta
    // Throttle the heavy markdown render (markdown-it + KaTeX + highlight.js)
    // instead of re-rendering the whole message on every token.
    scheduleStreamRender(reactiveAns)
  })
  unlisteners.set(answer.id, unlisten)

  const unlistenUsage = await listen<StreamUsagePayload>(`${eventName}-usage`, (event) => {
    const reactiveAns = findReactiveAnswer(answer.id)
    if (reactiveAns) applyUsage(reactiveAns, event.payload)
  })
  unlisteners.set(`${answer.id}-usage`, unlistenUsage)

  // Receive the actual context injected into the system prompt for the transparency banner
  const unlistenCtx = await listen<{ metadata: string; summary: string; fulltext: string; sections?: string }>(
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
      contextMode: answer.contextMode ?? 'none',
      usePdf: !!answer.usedPdf,
      sectionTitles: answer.sectionTitles ?? [],
      requestId,
    })
    const reactiveAns = findReactiveAnswer(answer.id)
    if (reactiveAns) {
      if (!reactiveAns.content && finalText) reactiveAns.content = finalText
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
      flushStreamRender(reactiveAns)
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
    const offUsage = unlisteners.get(`${answer.id}-usage`)
    if (offUsage) offUsage()
    unlisteners.delete(`${answer.id}-usage`)
    activeRequestIds.delete(answer.id)
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

// Estimated tokens of just THIS turn's user prompt (the text you typed),
// separate from the full \u2191 context (which also includes history + full text).
function currentTurnInputTokens(answer: AssistantAnswer): number {
  const conv = activeConversation.value
  if (!conv) return 0
  const group = conv.nodes.find(
    n => n.role === 'assistantGroup' && (n as AssistantGroupNode).answers.some(a => a.id === answer.id),
  ) as AssistantGroupNode | undefined
  if (!group) return 0
  const userNode = conv.nodes.find(n => n.role === 'user' && n.id === group.promptId)
  if (!userNode || userNode.role !== 'user') return 0
  return estimateTokens(userNode.content)
}

function applyUsage(answer: AssistantAnswer, usage: StreamUsagePayload) {
  if (typeof usage.input_tokens === 'number') answer.inputTokens = usage.input_tokens
  if (typeof usage.output_tokens === 'number') answer.outputTokens = usage.output_tokens
  if (typeof usage.total_tokens === 'number') answer.totalTokens = usage.total_tokens
  if (typeof usage.cache_hit_tokens === 'number') answer.cacheHitTokens = usage.cache_hit_tokens
  if (typeof usage.cost_usd === 'number' || usage.cost_usd === null) answer.costUsd = usage.cost_usd
}

// DeepSeek-style peak hours in Beijing time (UTC+8): 09:00–12:00 & 14:00–18:00.
function isPeakHour(date: Date): boolean {
  const minutes = ((date.getUTCHours() + 8) % 24) * 60 + date.getUTCMinutes()
  const h = minutes / 60
  return (h >= 9 && h < 12) || (h >= 14 && h < 18)
}

// Estimated CNY cost for models whose provider doesn't return a cost (e.g.
// DeepSeek), using the configured prices: cache-hit vs miss input + output, and
// peak/off-peak by the current time. Returns null when no CNY prices are set.
function estimatedCostCny(answer: AssistantAnswer): number | null {
  if (typeof answer.inputTokens !== 'number' || typeof answer.outputTokens !== 'number') return null
  const provider = ai.settings.providers.find(p => p.id === answer.providerId)
  const m = provider?.models.find(x => x.id === answer.modelId)
  if (!m || (m.input_price_per_million == null && m.output_price_per_million == null)) return null
  const peak = !!m.peak_pricing && isPeakHour(new Date())
  const inPrice = (peak && m.peak_input_price_per_million != null ? m.peak_input_price_per_million : m.input_price_per_million) ?? 0
  const outPrice = (peak && m.peak_output_price_per_million != null ? m.peak_output_price_per_million : m.output_price_per_million) ?? 0
  const cacheHit = answer.cacheHitTokens ?? 0
  const cacheMiss = Math.max(0, answer.inputTokens - cacheHit)
  const cacheHitPrice = m.cache_hit_input_price_per_million != null ? m.cache_hit_input_price_per_million : inPrice
  const cost = (cacheMiss / 1e6) * inPrice + (cacheHit / 1e6) * cacheHitPrice + (answer.outputTokens / 1e6) * outPrice
  return Number.isFinite(cost) && cost > 0 ? cost : null
}

function fmtCny(cny: number): string {
  if (cny < 0.01) return '<0.01'
  return cny.toFixed(cny < 1 ? 3 : 2)
}

function hasUsage(answer: AssistantAnswer) {
  return typeof answer.inputTokens === 'number' || typeof answer.outputTokens === 'number'
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

function answerSpeed(answer: AssistantAnswer) {
  if (!answer.startedAt || typeof answer.outputTokens !== 'number') return ''
  const end = answer.endedAt ?? performance.now()
  const seconds = Math.max(0.2, (end - answer.startedAt) / 1000)
  const speed = Math.round(answer.outputTokens / seconds)
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

function handleKeydown(e: KeyboardEvent) {
  // `isComposing` / keyCode 229 reliably detect an active IME composition,
  // avoiding the race-prone Date.now() heuristic that could send half-typed text.
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
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

// Left-rail message navigation: one tick per user message, hover previews the
// text, click scrolls to it.
const messageNav = computed(() =>
  (activeConversation.value?.nodes ?? [])
    .filter((n): n is UserNode => n.role === 'user')
    .map(n => ({ id: n.id, preview: n.content.trim() || '（空消息）' })),
)

function scrollToMessage(id: string) {
  const container = messagesEl.value
  if (!container) return
  const el = container.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(id)}"]`)
  if (!el) return
  const top = container.scrollTop + el.getBoundingClientRect().top - container.getBoundingClientRect().top - 12
  container.scrollTo({ top: Math.max(0, top), behavior: 'smooth' })
}

// The rail preview is teleported to <body> so no ancestor's `overflow: hidden`
// (the chat container / the rail itself) can clip it.
const navTip = ref<{ preview: string; x: number; y: number } | null>(null)
// Index of the hovered tick, driving the Codex-style "wave": the hovered tick is
// longest and its neighbours elongate progressively less with distance.
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
    flushStreamRender(ra)
  }
  const off = unlisteners.get(answerId)
  off?.()
  unlisteners.delete(answerId)
  const offUsage = unlisteners.get(`${answerId}-usage`)
  offUsage?.()
  unlisteners.delete(`${answerId}-usage`)
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
  if (sectionMenuRoot.value && !sectionMenuRoot.value.contains(e.target as Node)) {
    showSectionMenu.value = false
  }
}

watch(() => props.slug, async (slug) => {
  // Guard the whole (re)initialization: every contextMode/usePdf/section change
  // below is programmatic, so it must not be persisted (and thus not broadcast
  // to the other window). The `savedSel` we read here is the source of truth.
  restoringContext = true
  try {
    for (const off of unlisteners.values()) off()
    unlisteners.clear()
    clearAllStreamRenderTimers()
    showHistory.value = false
    showModelMenu.value = false
    activeConversation.value = null
    conversations.value = []
    activeAnswerTabs.value = {}
    input.value = ''
    applyFulltextReady(false, true)
    abstractAvailable.value = false
    summaryAvailable.value = false
    loadSections(slug)
    if (slug) {
      const savedSel = readContextSelection(slug)
      await Promise.all([
        refreshFulltextAvailability(slug, true),
        refreshAbstractAvailability(slug),
        refreshSummaryAvailability(slug),
      ])
      await loadConversations(slug)
      // Restore the last badge selection for this paper (shared across windows
      // via localStorage) so the sidebar tab and the standalone popup open in sync.
      if (savedSel) applyContextSelection(savedSel)
    }
  } finally {
    restoringContext = false
  }
}, { immediate: true })

// Persist the badge selection whenever the user changes it. `flush: 'sync'` is
// required so the write happens while `restoringContext` is still set during a
// programmatic change — otherwise the (async) callback would run after the flag
// was cleared and defeat the guard.
watch([contextMode, usePdf, selectedSectionTitles], () => {
  if (!restoringContext) persistContextSelection()
}, { deep: true, flush: 'sync' })

watch(() => allSelectableModels.value.map(modelKey).join('|'), ensureDefaultModels, { immediate: true })
watch(input, resizeTextarea)
watch(activeConversationIsMetadataExtraction, (isMetadataExtraction) => {
  if (isMetadataExtraction) showModelMenu.value = false
})

onMounted(async () => {
  await settingsStore.load()
  if (!ai.loaded) await ai.load()
  ensureDefaultModels()
  document.addEventListener('mousedown', closeFloating)
  window.addEventListener('focus', onWindowFocus)
  window.addEventListener('argus-paper-fulltext-updated', onPaperFulltextUpdated)
  window.addEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  window.addEventListener('argus-sections-updated', onSectionsUpdatedEvent)
  window.addEventListener('storage', onContextStorageSync)
  unlistenExtractionProgress = await listen<ExtractionProgressPayload>('extraction_progress', (event) => {
    if (event.payload.slug === props.slug && event.payload.ok) {
      applyFulltextReady(true)
    }
  })

  // `ai-meta-start` / `ai-meta-done` / `ai-meta-error` are broadcast globally
  // (Rust `app.emit`), so both the main window and the standalone popup receive
  // them. Only the main window should materialise the metadata-extraction
  // conversation — otherwise opening the same paper in the popup double-pushes
  // the group. Metadata extraction is always initiated from the main window.
  const isMainWindow = getCurrentWindow().label === 'main'
  if (isMainWindow) {
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
      scheduleStreamRender(ra)
    })
    unlisteners.set(answer_id, unlisten)
    const unlistenUsage = await listen<StreamUsagePayload>(`paper-ai-chat-${answer_id}-usage`, (event) => {
      const ra = findReactiveAnswer(answer_id)
      if (ra) applyUsage(ra, event.payload)
    })
    unlisteners.set(`${answer_id}-usage`, unlistenUsage)
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
  unlistenMetaError = await listen<{ slug: string; answer_id: string; error: string }>('ai-meta-error', (ev) => {
    if (ev.payload.slug !== props.slug) return
    const ra = findReactiveAnswer(ev.payload.answer_id)
    if (ra) {
      ra.error = true
      ra.errorText = ev.payload.error
      if (!ra.content) ra.content = `提取失败：${ev.payload.error}`
    }
    finaliseMetaAnswer(ev.payload.answer_id)
  })
  }
  messagesEl.value?.addEventListener('copy-code', (e: Event) => {
    navigator.clipboard.writeText((e.target as HTMLElement).textContent ?? '').catch(() => {})
  })
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeFloating)
  window.removeEventListener('focus', onWindowFocus)
  window.removeEventListener('argus-paper-fulltext-updated', onPaperFulltextUpdated)
  window.removeEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  window.removeEventListener('argus-sections-updated', onSectionsUpdatedEvent)
  window.removeEventListener('storage', onContextStorageSync)
  unlistenExtractionProgress?.()
  unlistenMetaStart?.()
  unlistenMetaDone?.()
  unlistenMetaError?.()
  for (const off of unlisteners.values()) off()
  unlisteners.clear()
  clearAllStreamRenderTimers()
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

interface CtxFlags {
  metadata: boolean
  summary: boolean
  fulltext: boolean
  sections: boolean
  pdf: boolean
}

// Which context types a single answer actually carried.
function answerContextFlags(ans: AssistantAnswer): CtxFlags {
  const c = ans.contextContent
  if (c) {
    return {
      metadata: !!c.metadata?.trim(),
      summary: !!c.summary?.trim(),
      fulltext: !!c.fulltext?.trim(),
      sections: !!c.sections?.trim(),
      pdf: !!ans.usedPdf,
    }
  }
  const mode = ans.contextMode ?? 'none'
  return {
    metadata: mode === 'metadata',
    summary: mode === 'summary' || mode === 'summary+fulltext',
    fulltext: mode === 'fulltext' || mode === 'summary+fulltext',
    sections: !!ans.sectionTitles?.length,
    pdf: !!ans.usedPdf,
  }
}

// Context this turn introduces for the FIRST time (vs. earlier turns). The model
// still receives the context every turn (it's stateless), but the badge is only
// shown on the message that first added it — like an attachment shown once, not
// re-announced on every follow-up.
function newlyAddedContext(userNodeId: string): CtxFlags {
  const empty: CtxFlags = { metadata: false, summary: false, fulltext: false, sections: false, pdf: false }
  const conv = activeConversation.value
  if (!conv) return empty
  const prev = { ...empty }
  for (const node of conv.nodes) {
    if (node.role !== 'assistantGroup') continue
    const ans = node.answers[0]
    if (!ans) continue
    const flags = answerContextFlags(ans)
    if (node.promptId === userNodeId) {
      return {
        metadata: flags.metadata && !prev.metadata,
        summary: flags.summary && !prev.summary,
        fulltext: flags.fulltext && !prev.fulltext,
        sections: flags.sections && !prev.sections,
        pdf: flags.pdf && !prev.pdf,
      }
    }
    prev.metadata = prev.metadata || flags.metadata
    prev.summary = prev.summary || flags.summary
    prev.fulltext = prev.fulltext || flags.fulltext
    prev.sections = prev.sections || flags.sections
    prev.pdf = prev.pdf || flags.pdf
  }
  return empty
}

function hasContextBanner(userNodeId: string): boolean {
  const f = newlyAddedContext(userNodeId)
  return f.metadata || f.summary || f.fulltext || f.sections || f.pdf
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
        <Icon icon="fluent:sparkle-24-regular" width="34" height="34" />
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
          <Icon icon="fluent:add-24-regular" width="15" height="15" />
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
              <Icon icon="fluent:delete-24-regular" width="13" height="13" />
            </button>
          </div>
          <div v-if="conversations.length === 0" class="history-empty">暂无历史对话</div>
        </div>
      </aside>

      <section class="chat-main">
        <header class="ai-header" :data-tauri-drag-region="props.standalone ? '' : undefined">
          <div class="header-main" :data-tauri-drag-region="props.standalone ? '' : undefined">
            <span class="spark-icon">
              <Icon icon="fluent:sparkle-24-regular" width="15" height="15" />
            </span>
            <span class="header-title">{{ activeTitle }}</span>
          </div>

          <div class="header-actions">
            <button class="icon-btn" title="新建对话" @click="startNewConversation(true)">
              <Icon icon="fluent:add-24-regular" width="15" height="15" />
            </button>
            <button v-if="!props.standalone" class="icon-btn" title="在独立窗口打开" @click="openPopupWindow">
              <Icon icon="fluent:open-24-regular" width="14" height="14" />
            </button>
            <button v-if="!props.standalone" class="icon-btn" title="对话历史" @click="showHistory = true">
              <Icon icon="fluent:chat-24-regular" width="16" height="16" />
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
            <Icon class="chevron" :class="{ open: showModelMenu }" icon="fluent:chevron-down-24-regular" width="12" height="12" />
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

        <div class="messages-wrap">
        <nav
          v-if="!activeConversationIsMetadataExtraction && messageNav.length > 1"
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
        <div
          ref="messagesEl"
          class="messages"
          :class="{ 'messages--no-floating-model': activeConversationIsMetadataExtraction }"
          @click="onMsgContainerClick"
        >
        <div v-if="!activeConversation?.nodes.length" class="empty-chat">
          <div class="empty-orb">
            <Icon icon="fluent:sparkle-24-regular" width="22" height="22" />
          </div>
          <p>基于这篇论文开始新对话</p>
          <span>选择模型，开始与这篇论文对话。</span>
        </div>

        <template v-for="node in activeConversation?.nodes ?? []" :key="node.id">
          <div v-if="node.role === 'user'" class="user-row" :data-node-id="node.id">
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
                    <span v-if="newlyAddedContext(node.id).metadata" class="ctx-pill ctx-meta">元数据</span>
                    <span v-if="newlyAddedContext(node.id).summary" class="ctx-pill ctx-summary">AI 总结</span>
                    <span v-if="newlyAddedContext(node.id).sections" class="ctx-pill ctx-sections">章节</span>
                    <span v-if="newlyAddedContext(node.id).fulltext" class="ctx-pill ctx-fulltext">全文</span>
                    <span v-if="newlyAddedContext(node.id).pdf" class="ctx-pill ctx-pdf">PDF</span>
                    <Icon class="ctx-chevron" :class="{ open: expandedContextId === node.id }" icon="fluent:chevron-down-24-regular" width="11" height="11" />
                  </button>
                  <div v-if="expandedContextId === node.id" class="ctx-preview">
                    <template v-if="getFirstAnswer(node.id)?.contextContent">
                      <div v-if="newlyAddedContext(node.id).metadata && getFirstAnswer(node.id)!.contextContent!.metadata" class="ctx-section">
                        <div class="ctx-section-label">元数据</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.metadata }}</pre>
                      </div>
                      <div v-if="newlyAddedContext(node.id).summary && getFirstAnswer(node.id)!.contextContent!.summary" class="ctx-section">
                        <div class="ctx-section-label">AI 总结</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.summary }}</pre>
                      </div>
                      <div v-if="newlyAddedContext(node.id).sections && getFirstAnswer(node.id)!.contextContent!.sections" class="ctx-section">
                        <div class="ctx-section-label">章节</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.sections }}</pre>
                      </div>
                      <div v-if="newlyAddedContext(node.id).fulltext && getFirstAnswer(node.id)!.contextContent!.fulltext" class="ctx-section">
                        <div class="ctx-section-label">全文</div>
                        <pre class="ctx-preview-text">{{ getFirstAnswer(node.id)!.contextContent!.fulltext }}</pre>
                      </div>
                      <div v-if="newlyAddedContext(node.id).pdf && !newlyAddedContext(node.id).fulltext" class="ctx-section">
                        <pre class="ctx-preview-text">PDF 文件已直接发送给模型</pre>
                      </div>
                    </template>
                    <div v-else class="ctx-loading">{{ getFirstAnswer(node.id)?.streaming ? '等待后端响应…' : '暂无上下文记录（旧对话不支持）' }}</div>
                  </div>
                </div>
                <div v-if="node.attachments && node.attachments.length" class="user-attachments">
                  <button
                    v-for="att in node.attachments"
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
                <div class="user-bubble">{{ node.content }}</div>
                <div class="msg-footer user-footer">
                  <div class="msg-actions">
                    <button class="action-btn" :class="{ done: copiedIds.has(node.id) }" title="复制" @click="copyText(node.id, node.content)">
                      <Icon v-if="copiedIds.has(node.id)" icon="fluent:checkmark-24-regular" width="12" height="12" />
                      <Icon v-else icon="fluent:copy-24-regular" width="12" height="12" />
                    </button>
                    <button class="action-btn" title="编辑" :disabled="hasStreaming" @click="startEdit(node)">
                      <Icon icon="fluent:edit-24-regular" width="12" height="12" />
                    </button>
                    <button class="action-btn danger" title="删除该对话" :disabled="hasStreaming" @click="deleteExchange(node.id)">
                      <Icon icon="fluent:delete-24-regular" width="12" height="12" />
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
                    <Icon class="reasoning-chevron" icon="fluent:chevron-right-24-regular" width="11" height="11" />
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
                    <div v-if="answer.content" v-html="renderMarkdown(answer.displayContent ?? answer.content)" />
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
                    <Icon v-if="copiedIds.has(answer.id)" icon="fluent:checkmark-24-regular" width="12" height="12" />
                    <Icon v-else icon="fluent:copy-24-regular" width="12" height="12" />
                  </button>
                  <button class="action-btn" :disabled="answer.streaming || hasStreaming" title="重新生成" @click="regenerate(node, answer)">
                    <Icon icon="fluent:arrow-sync-24-regular" width="12" height="12" />
                  </button>
                  <button class="action-btn danger" :disabled="answer.streaming || hasStreaming" title="删除该回答" @click="deleteAnswer(node, answer)">
                    <Icon icon="fluent:delete-24-regular" width="12" height="12" />
                  </button>
                </div>
                <div v-if="hasUsage(answer) || answer.error" class="msg-usage">
                  <span v-if="currentTurnInputTokens(answer) > 0" class="usage-tokens usage-turn-input" title="本轮你输入的内容（估算 tokens，不含历史与全文）">
                    <Icon icon="fluent:edit-24-regular" width="10" height="10" />
                    {{ formatTokenCount(currentTurnInputTokens(answer)) }}
                  </span>
                  <span v-if="typeof answer.inputTokens === 'number'" class="usage-tokens" title="上下文输入 tokens（含历史与全文）">↑{{ formatTokenCount(answer.inputTokens) }}</span>
                  <span v-if="typeof answer.outputTokens === 'number'" class="usage-tokens" title="本次输出 tokens">↓{{ formatTokenCount(answer.outputTokens) }}</span>
                  <span v-if="answerSpeed(answer)" class="msg-speed">{{ answerSpeed(answer) }}</span>
                  <span v-if="answer.costUsd != null && formatCostCny(answer.costUsd)" class="usage-cost" :title="`约 ¥${formatCostCny(answer.costUsd)} / $${answer.costUsd.toFixed(6)}`">¥{{ formatCostCny(answer.costUsd) }}</span>
                  <span v-else-if="answer.costUsd == null && estimatedCostCny(answer) != null" class="usage-cost usage-cost-est" :title="`按配置单价估算（含缓存命中/峰谷），约 ¥${estimatedCostCny(answer)!.toFixed(6)}`">≈¥{{ fmtCny(estimatedCostCny(answer)!) }}</span>
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
          <div ref="sectionMenuRoot" class="context-section-wrap">
            <button
              class="context-btn context-btn-sections"
              :class="{ active: sectionsActive }"
              :disabled="!availableSections.length"
              :title="availableSections.length ? '按章节作为上下文（可多选）' : '尚未识别到章节，请先在「章节」页签识别'"
              @click="toggleSectionMenu"
            >
              <span class="context-btn-label">章节</span>
              <span v-if="sectionsActive" class="context-count">{{ selectedSectionTitles.length }}</span>
              <Icon class="context-caret" :class="{ open: showSectionMenu }" icon="fluent:chevron-down-24-regular" width="9" height="9" />
            </button>
            <Transition name="section-menu">
              <div v-if="showSectionMenu" class="section-menu">
                <div class="section-menu-head">
                  <span class="section-menu-title">选择章节作为上下文</span>
                  <div class="section-menu-actions">
                    <button class="section-menu-link" @click="selectAllSections">全选</button>
                    <button class="section-menu-link" @click="clearSelectedSections">清空</button>
                  </div>
                </div>
                <div class="section-menu-list">
                  <label
                    v-for="(sec, i) in availableSections"
                    :key="i"
                    class="section-menu-item"
                    :class="`level-${sec.level}`"
                    :title="sec.title"
                  >
                    <input
                      type="checkbox"
                      :checked="selectedSectionTitles.includes(sec.title)"
                      @change="toggleSection(i)"
                    />
                    <span class="section-menu-item-title">{{ sec.title }}</span>
                    <span v-if="sec.page > 0" class="section-menu-item-page">p.{{ sec.page }}</span>
                  </label>
                </div>
              </div>
            </Transition>
          </div>
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
            class="composer-input"
            rows="1"
            placeholder="问这篇论文里的任何问题…"
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
          <div class="composer-toolbar">
            <button class="toolbar-btn" title="新建对话" @click="startNewConversation(true)">
              <Icon icon="fluent:compose-24-regular" width="15" height="15" />
            </button>
            <button class="toolbar-btn" title="上传图片或 PDF" @click="openFilePicker">
              <Icon icon="fluent:attach-24-regular" width="15" height="15" />
            </button>

            <!-- Reasoning / thinking mode picker -->
            <div ref="reasoningRoot" class="reasoning-picker">
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

            <span class="toolbar-spacer" />

            <button v-if="hasStreaming" class="send-btn stop-btn" title="停止生成" @click="stopAllStreaming">
              <Icon icon="fluent:stop-24-filled" width="15" height="15" />
            </button>
            <button v-else class="send-btn" :class="{ active: canSend }" :disabled="!canSend" @click="sendMessage">
              <Icon icon="fluent:send-24-regular" width="15" height="15" />
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
                <Icon icon="fluent:dismiss-24-regular" width="15" height="15" />
              </button>
            </div>

            <button class="new-chat-btn" @click="startNewConversation(true)">
              <Icon icon="fluent:add-24-regular" width="15" height="15" />
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
                  <Icon icon="fluent:delete-24-regular" width="13" height="13" />
                </button>
              </div>
              <div v-if="conversations.length === 0" class="history-empty">暂无历史对话</div>
            </div>
          </aside>
        </div>
      </section>
    </template>

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
  height: var(--content-header-height);
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
  top: calc(var(--content-header-height) + 18px);
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
  padding: calc(var(--content-header-height) + 18px) 12px 12px;
}
.messages--no-floating-model {
  padding-top: 12px;
}
.standalone .messages {
  padding: 24px 20px 0;
}
.empty-chat {
  min-height: calc(100% - var(--content-header-height));
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
.action-btn.danger:hover:not(:disabled) { color: #ef4444; background: color-mix(in srgb, #ef4444 12%, transparent); }
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
.usage-turn-input { display: inline-flex; align-items: center; gap: 2px; }
.msg-speed { color: color-mix(in srgb, var(--accent) 74%, var(--text-tertiary)); }
.usage-cost {
  color: var(--text-secondary);
  font-weight: 500;
  margin-left: 2px;
}
.usage-cost-est { color: var(--text-tertiary); font-weight: 400; }
.error-badge { color: #ef4444; }

.composer {
  position: relative;
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

.context-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 15px;
  height: 15px;
  padding: 0 4px;
  border-radius: 8px;
  font-size: 10px;
  font-weight: 700;
  line-height: 1;
  color: #fff;
  background: var(--accent);
}

/* ── Section (chapter) context dropdown — opens upward, spans the composer ── */
.context-section-wrap { display: inline-flex; }
/* Reserve enough width up front so adding the count badge doesn't shift layout. */
.context-btn-sections {
  min-width: 74px;
  justify-content: center;
}
.context-btn-label { flex-shrink: 0; }
.context-caret {
  flex-shrink: 0;
  opacity: 0.55;
  transition: transform 0.16s ease;
}
.context-caret.open { transform: rotate(180deg); }

.section-menu {
  position: absolute;
  bottom: 100%;
  left: 10px;
  right: 10px;
  margin-bottom: 6px;
  z-index: 40;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.18);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}
.section-menu-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border-subtle);
}
.section-menu-title { font-size: 11.5px; font-weight: 600; color: var(--text-secondary); }
.section-menu-actions { display: flex; gap: 8px; }
.section-menu-link {
  font-size: 11px;
  color: var(--accent);
  cursor: pointer;
}
.section-menu-link:hover { text-decoration: underline; }
.section-menu-list {
  max-height: 320px;
  overflow-y: auto;
  padding: 4px;
}
.section-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 7px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}
.section-menu-item:hover { background: var(--bg-hover); }
/* Custom checkbox: the native control renders as an ugly black box in some
   themes; draw our own rounded box with an accent check instead. */
.section-menu-item input {
  appearance: none;
  -webkit-appearance: none;
  flex-shrink: 0;
  width: 15px;
  height: 15px;
  margin: 0;
  border: 1.5px solid var(--border-default);
  border-radius: 4px;
  background: var(--bg-primary);
  cursor: pointer;
  position: relative;
  transition: background 0.12s, border-color 0.12s;
}
.section-menu-item input:hover { border-color: color-mix(in srgb, var(--accent) 55%, var(--border-default)); }
.section-menu-item input:checked {
  background: var(--accent);
  border-color: var(--accent);
}
.section-menu-item input:checked::after {
  content: '';
  position: absolute;
  left: 4px;
  top: 1px;
  width: 4px;
  height: 8px;
  border: solid #fff;
  border-width: 0 2px 2px 0;
  transform: rotate(45deg);
}
.section-menu-item.level-2 { padding-left: 20px; color: var(--text-secondary); }
.section-menu-item.level-3 { padding-left: 34px; color: var(--text-tertiary); font-size: var(--font-size-xs); }
.section-menu-item-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.section-menu-item-page {
  flex-shrink: 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}
.section-menu-enter-active,
.section-menu-leave-active { transition: opacity 0.12s ease, transform 0.12s ease; }
.section-menu-enter-from,
.section-menu-leave-to { opacity: 0; transform: translateY(6px); }

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
.attachment-row {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 8px 12px 0;
}
.attachment-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 180px;
  padding: 4px 6px;
  border-radius: 8px;
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
  margin-left: 2px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
}
.attachment-remove:hover {
  background: rgba(0, 0, 0, 0.08);
  color: var(--text-primary);
}
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
.ctx-sections{ background: color-mix(in srgb, #2563eb 14%, transparent); color: #1d4ed8; }
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
  border: 1px solid var(--border-default);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 11px;
  cursor: pointer;
  transition: background 0.1s, border-color 0.1s;
}
.user-attachment:hover {
  background: var(--accent-light);
  border-color: var(--accent);
  color: var(--accent);
}
.user-attachment.pdf {
  background: #fff0f0;
  border-color: #f0c0c0;
  color: #8b1e1e;
}
.user-attachment.pdf:hover {
  background: #ffe0e0;
  border-color: #e0a0a0;
}
.user-attachment-thumb {
  width: 18px;
  height: 18px;
  object-fit: cover;
  border-radius: 4px;
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
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.78);
  backdrop-filter: blur(2px);
}
.lightbox-image {
  max-width: 92vw;
  max-height: 92vh;
  border-radius: 10px;
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.35);
}
.lightbox-pdf {
  width: 92vw;
  height: 92vh;
  border-radius: 10px;
  background: #fff;
}
.lightbox-close {
  position: absolute;
  top: 16px;
  right: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  border: none;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.12);
  color: #fff;
  cursor: pointer;
  transition: background 0.15s;
}
.lightbox-close:hover {
  background: rgba(255, 255, 255, 0.22);
}
</style>
