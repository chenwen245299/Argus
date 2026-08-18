<script setup lang="ts">
import { modelOffer, modelSizeLabel } from '../../utils/modelOffers'
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useAiStore, type ModelOption } from '../../stores/ai'
import { useSettingsStore } from '../../stores/settings'
import MarkdownBody from '../MarkdownBody.vue'
import { svgStringToPngBlob } from '../../utils/svgToPng'
import { copyPngBlobToClipboard } from '../../utils/clipboard'
import { beginDragSelectionGuard, endDragSelectionGuard } from '../../utils/dragSelectionGuard'
import type { ChatContentPart, ChatMessage, PaperMeta, AgentWritePreview } from '../../types'
import WriteConfirmCard from '../WriteConfirmCard.vue'
import { buildToolExchangeMessages } from '../../utils/agentHistory'
import { askAiText } from '../../stores/translationHistory'
import { estimateCostCny } from '../../utils/modelPricing'

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
  source?: 'chat' | 'metadataExtraction'
  /** What the agent did to answer: one entry per tool call, in order. */
  steps?: AgentStep[]
  /** Servers the user configured that would not start this turn. */
  serverErrors?: string[]
  /** The model ran out of tool rounds before it was finished. */
  limitHit?: { rounds: number; max: number }
}

/** One tool call the agent made, as shown in the trail above an answer. */
interface AgentStep {
  tool: string
  server?: string
  args: string
  argsJson?: string
  ok?: boolean
  chars?: number
  /** The tool result, as the model received it. Shown when the row is opened. */
  preview?: string
  /** Whether the model's own copy was cut to fit its context budget. */
  truncated?: boolean
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

/** Expanded tool rows, keyed by `${answerId}:${index}`. Collapsed by default —
 *  the trail is a summary, and the payloads behind it are long. */
const expandedSteps = ref(new Set<string>())
function stepKey(answerId: string, i: number) { return `${answerId}:${i}` }
function toggleStep(answerId: string, i: number) {
  const k = stepKey(answerId, i)
  // Replace the Set rather than mutate it: Vue does not track Set mutations
  // deeply enough to re-render the bindings that depend on this.
  const next = new Set(expandedSteps.value)
  if (next.has(k)) next.delete(k)
  else next.add(k)
  expandedSteps.value = next
}

/** True while any step of this answer is still waiting on its result. */
function agentRunning(answer: AssistantAnswer): boolean {
  return !!answer.steps?.some(s => s.ok === undefined)
}

/** Compact size for a tool result, so a long trail stays scannable. */
function formatChars(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n)
}

// ── Prompt-cache keepalive ──────────────────────────────────────────────────
// The backend holds the last agent answer's prefix warm for an hour so a
// follow-up asked much later still bills at the cache-hit rate. It spends a
// little in the background doing that, so the conversation it is holding is
// marked rather than left invisible. Same status event the library chat reads —
// only one conversation is ever held at a time, app-wide.
interface KeepaliveStatus {
  active: boolean
  conversationId?: string | null
  model?: string
  pings?: number
  /** When the hour of inactivity runs out, in epoch ms. */
  stopsAtMs?: number
  intervalSeconds?: number
  reason?: 'idle' | 'left' | 'failing' | 'disarmed'
}
const keepalive = ref<KeepaliveStatus>({ active: false })
/** Ticks once a minute purely so the remaining-time readout stays honest. */
const nowMs = ref(Date.now())
let keepaliveClock: ReturnType<typeof setInterval> | null = null
let unlistenKeepalive: UnlistenFn | null = null

function isCacheWarm(convId: string | null | undefined) {
  return !!convId && keepalive.value.active && keepalive.value.conversationId === convId
}

const keepaliveTitle = computed(() => {
  const k = keepalive.value
  const every = k.intervalSeconds ? Math.round(k.intervalSeconds / 60) : 5
  const left = k.stopsAtMs ? Math.max(0, Math.round((k.stopsAtMs - nowMs.value) / 60000)) : null
  return [
    `正在保持这段对话的上下文缓存，每 ${every} 分钟续期一次。`,
    '下次提问会命中缓存，按缓存价计费，而不是重读整段对话。',
    k.pings ? `已续期 ${k.pings} 次` : '尚未续期',
    left !== null ? `· 约 ${left} 分钟后自动停止` : '',
    '关掉这个窗口也会停止。',
  ].filter(Boolean).join('\n')
})
const previewImage = ref<string | null>(null)
const previewPdf = ref<string | null>(null)
const modelMenuRoot = ref<HTMLElement | null>(null)
const unlisteners = new Map<string, UnlistenFn>()
// Maps answer.id -> backend request_id, so stopAllStreaming can tell the backend
// to truly cancel the in-flight HTTP request (stop the provider generating/billing).
const activeRequestIds = new Map<string, string>()
// Answers that are streaming right now, mapped to the paper + conversation that
// owns them. Holding the conversation here keeps it alive and reactive after the
// user switches to another paper, so a generation started on paper A keeps
// streaming into A's conversation in the background instead of being dropped.
const streamOwners = new Map<string, { slug: string; conv: Conversation }>()
// Per-paper view state, snapshotted when leaving a paper and restored when
// coming back, so switching tabs doesn't drop the user back into a blank chat.
interface PaperSession {
  conv: Conversation | null
  tabs: Record<string, string>
  input: string
}
const sessions = new Map<string, PaperSession>()
const abstractAvailable = ref(false)
const activeAnswerTabs = ref<Record<string, string>>({})
let unlistenMetaStart: UnlistenFn | null = null
let unlistenMetaDone: UnlistenFn | null = null
let unlistenMetaError: UnlistenFn | null = null

// ── Panel width ───────────────────────────────────────────────────────────────
// Docked in the right sidebar this panel can be narrow, and the usage row
// (turn tokens, context tokens, output tokens, cache hit, throughput, cost)
// then wraps onto a second line. Throughput is the least load-bearing of those
// numbers, so it is the one that drops out when there isn't room. The popup
// window is always wide enough, so it never hides.
const rootRef = ref<HTMLElement | null>(null)
const panelWidth = ref(0)
let panelResizeObserver: ResizeObserver | null = null

/**
 * Below this the usage row no longer fits on one line. Measured: the row wraps
 * at roughly 330px of card width, and the card is the panel minus `.messages`'
 * 2×12px padding — so the sidebar's 350px minimum lands just inside the wrap,
 * which is what put the cost on its own line. 380 clears it with enough headroom
 * for the longer numbers (↑158.2k, ~120 tok/s, ≈¥12.34). The sidebar can be
 * dragged from 350 to 560, so this stays reachable.
 */
const SPEED_MIN_WIDTH = 380
const showAnswerSpeed = computed(() => props.standalone || panelWidth.value >= SPEED_MIN_WIDTH)

// Copy state for message actions
const copiedIds = ref(new Set<string>())

// Inline edit state for user messages
const editingNodeId = ref<string | null>(null)
const editingText = ref('')

// ── Agent write confirmations ────────────────────────────────────────────────
// The agent's one writing tool (create_paper_note) parks its request in the
// backend and waits. The card above the composer shows the first one; the rest
// queue behind it, since several models can be answering at once. Nothing is
// written until answerWrite sends an approval back — the backend defaults every
// other outcome (timeout, stop, this view closing) to "do not write".
interface PendingWrite { requestId: string; preview: AgentWritePreview }
const pendingWrites = ref<PendingWrite[]>([])
const currentWrite = computed<PendingWrite | null>(() => pendingWrites.value[0] ?? null)

function answerWrite(approved: boolean) {
  const pending = currentWrite.value
  if (!pending) return
  // Drop it from the queue first so the button can't answer the same request twice.
  pendingWrites.value = pendingWrites.value.slice(1)
  invoke('resolve_agent_write', { requestId: pending.requestId, approved }).catch(() => {})
}

/** Take a request off the queue without answering — the backend already stopped
 *  waiting for it (timeout, or the generation was stopped). */
function dismissWrite(requestId: string) {
  pendingWrites.value = pendingWrites.value.filter(w => w.requestId !== requestId)
}

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

// Access an answer through Vue's reactive proxy chain (fixes reactivity bug).
// A streaming answer is looked up in its own conversation first, so a background
// generation still lands in the right place after the user switched papers.
function findReactiveAnswer(answerId: string): AssistantAnswer | null {
  const owner = streamOwners.get(answerId)?.conv
  const scopes = [owner, activeConversation.value].filter((c): c is Conversation => !!c)
  for (const conv of scopes) {
    for (const node of conv.nodes) {
      if (node.role === 'assistantGroup') {
        const ans = node.answers.find(a => a.id === answerId)
        if (ans) return ans
      }
    }
  }
  return null
}

// Whether an answer belongs to the conversation currently on screen. Background
// answers must not scroll (or otherwise disturb) the paper the user is reading.
function isAnswerVisible(answerId: string): boolean {
  const conv = activeConversation.value
  if (!conv) return false
  return conv.nodes.some(n => n.role === 'assistantGroup' && n.answers.some(a => a.id === answerId))
}

// The conversation object behind a still-streaming conversation id, if any.
function liveConversationById(id: string): Conversation | null {
  for (const { conv } of streamOwners.values()) {
    if (conv.id === id) return conv
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

// ── Persisting the agent trail ──────────────────────────────────────────────
// Tool results are kept so an old answer can still be audited after a reload —
// opening a step then shows what the model actually got, not an empty box. The
// caps below are runaway guards rather than savings: every conversation for a
// paper shares one file, so one pathological run (hundreds of rounds, each
// result half a megabyte) would make that file slow to rewrite while the next
// answer is still streaming into it. Real results sit far below these.

/** Per tool result. */
const PERSIST_STEP_CHARS = 60_000
/** Per answer, across all of its tool calls. */
const PERSIST_ANSWER_CHARS = 400_000

/** Trim one answer's tool payloads down to what is worth writing to disk. */
function persistableSteps(steps?: AgentStep[]): AgentStep[] | undefined {
  if (!steps) return undefined
  let budget = PERSIST_ANSWER_CHARS
  return steps.map(step => {
    if (!step.preview) return step
    const room = Math.min(PERSIST_STEP_CHARS, budget)
    if (room <= 0) {
      const { preview: _drop, ...rest } = step
      return rest
    }
    budget -= Math.min(step.preview.length, room)
    if (step.preview.length <= room) return step
    return { ...step, preview: step.preview.slice(0, room) }
  })
}

/** A conversation as it goes to disk: no throttled render copies, tool payloads
 *  capped. */
function persistableConversation(conv: Conversation): Conversation {
  const clone = cloneConversation(conv)
  for (const node of clone.nodes) {
    if (node.role !== 'assistantGroup') continue
    for (const answer of node.answers) {
      delete answer.displayContent
      answer.steps = persistableSteps(answer.steps)
    }
  }
  return clone
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
    .map(persistableConversation)
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
  // A background generation for this paper may still be flushing to disk; read
  // after it lands so the history list isn't one answer behind.
  await (backgroundSaves.get(slug) ?? Promise.resolve()).catch(() => {})
  let list: Conversation[] = []
  try {
    list = normalizeConversations(await invoke<unknown>('get_paper_ai_conversations', { slug }), slug)
  } catch {
    list = []
  }
  // The user may have switched papers again while this was loading.
  if (props.slug !== slug) return
  conversations.value = list
  await importLegacyHistory(slug)
  if (props.slug !== slug) return
  restoreSession(slug)
}

// Papers whose view state is kept in memory. Old entries are dropped, but never
// one whose answer is still generating: that object is the live target.
const MAX_CACHED_SESSIONS = 30

function hasLiveStreamFor(slug: string): boolean {
  for (const owner of streamOwners.values()) {
    if (owner.slug === slug) return true
  }
  return false
}

function rememberSession(slug: string) {
  // Re-insert so the Map's iteration order stays least-recently-used first.
  sessions.delete(slug)
  sessions.set(slug, {
    conv: activeConversation.value,
    tabs: { ...activeAnswerTabs.value },
    input: input.value,
  })
  while (sessions.size > MAX_CACHED_SESSIONS) {
    const evictable = [...sessions.keys()].find(k => !hasLiveStreamFor(k))
    if (!evictable) break
    sessions.delete(evictable)
  }
}

// Bring back the conversation the user was last looking at for this paper —
// including one that is still generating — instead of opening a blank chat.
function restoreSession(slug: string) {
  const session = sessions.get(slug)
  sessions.delete(slug)
  if (!session?.conv) {
    startNewConversation(false)
    return
  }
  activeConversation.value = session.conv
  activeAnswerTabs.value = session.tabs
  input.value = session.input
  showHistory.value = false
  nextTick(() => scrollToBottom(true))
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
  // If this conversation still has a live generation, reopen the streaming
  // object itself — a snapshot would stop updating.
  activeConversation.value = liveConversationById(id) ?? cloneConversation(conv)
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
  if (!props.slug || !conv) return
  persistConversationFor(props.slug, conv)
}

// Persist `conv` under `slug`, which is not necessarily the paper on screen: a
// background generation can finish long after the user switched away.
function persistConversationFor(slug: string, conv: Conversation) {
  if (conv.nodes.length === 0) return
  conv.updatedAt = nowIso()
  conv.title = isMetadataExtractionConversation(conv)
    ? 'AI 元数据提取'
    : firstUserTitle(conv.nodes) || conv.title || '新对话'
  if (slug !== props.slug) {
    queueBackgroundSave(slug, conv)
    return
  }
  const idx = conversations.value.findIndex(c => c.id === conv.id)
  if (idx >= 0) conversations.value[idx] = cloneConversation(conv)
  else conversations.value.unshift(cloneConversation(conv))
  conversations.value.sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime())
  saveConversationsToPaper(slug).catch(() => {})
  saveLegacyActiveHistory(slug, conv).catch(() => {})
}

// Saving for a paper this view doesn't have loaded is a read-modify-write on its
// conversation file, so writes are chained per paper — two models finishing at
// almost the same time would otherwise clobber each other.
const backgroundSaves = new Map<string, Promise<void>>()

function queueBackgroundSave(slug: string, conv: Conversation) {
  // Snapshot now: the live conversation keeps mutating while the write awaits.
  const snapshot = persistableConversation(conv)
  const prev = backgroundSaves.get(slug) ?? Promise.resolve()
  const next: Promise<void> = prev
    .catch(() => {})
    .then(() => saveConversationForOtherPaper(slug, snapshot))
    .catch(() => {})
    .finally(() => {
      if (backgroundSaves.get(slug) === next) backgroundSaves.delete(slug)
    })
  backgroundSaves.set(slug, next)
}

async function saveConversationForOtherPaper(slug: string, snapshot: Conversation) {
  let list: Conversation[] = []
  try {
    list = normalizeConversations(await invoke<unknown>('get_paper_ai_conversations', { slug }), slug)
  } catch {
    list = []
  }
  const idx = list.findIndex(c => c.id === snapshot.id)
  if (idx >= 0) list[idx] = snapshot
  else list.unshift(snapshot)
  const persisted = list
    .filter(c => c.nodes.length > 0)
    .sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime())
  await invoke('save_paper_ai_conversations', { slug, conversations: persisted })
  await invoke('save_chat_history', { slug, messages: flattenConversation(snapshot) })
}

async function saveLegacyActiveHistory(slug: string, conv: Conversation) {
  const messages = flattenConversation(conv)
  await invoke('save_chat_history', { slug, messages })
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

/** The FREE / 折扣 badge for a picker row, from the provider's own price list. */
function sizeOf(model: ModelOption) {
  return modelSizeLabel(model.paramBillions)
}

function offerOf(model: ModelOption) {
  return modelOffer({
    is_free: model.isFree,
    discount_percent: model.discountPercent,
    discount_windows: model.discountWindows,
  })
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
        // Replay this turn's tool calls + results (as native tool messages)
        // before its answer, so a follow-up reuses them instead of re-fetching.
        messages.push(...buildToolExchangeMessages(answer.steps, answer.id))
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
  // Pin the paper up front: the user can switch tabs while this turn is being
  // set up, and the answer must still belong to the paper they asked about.
  const slug = props.slug
  const text = input.value.trim()
  activeConversation.value = activeConversation.value ?? createBlankConversation(slug)
  // Read it back so `conv` is the reactive proxy, not the raw object: streaming
  // mutations (possibly from the background) must be tracked by Vue.
  const conv = activeConversation.value

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
    answers: selectedModels.value.map(model => modelToAnswer(model)),
  }
  if (group.answers[0]) setActiveAnswer(group.id, group.answers[0].id)
  conv.nodes.push(userNode, group)
  conv.title = firstUserTitle(conv.nodes) || conv.title
  input.value = ''
  attachments.value = []
  persistConversationFor(slug, conv)
  await nextTick()
  resizeTextarea()
  scrollToBottom(true)

  const history = buildHistoryUntil(conv, group.id)
  await Promise.all(group.answers.map(answer => streamAnswer(slug, conv, answer, history)))
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
  }
}

async function regenerate(group: ChatNode, answer: AssistantAnswer) {
  if (group.role !== 'assistantGroup' || !activeConversation.value || !props.slug || answer.streaming) return
  const slug = props.slug
  const conv = activeConversation.value
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
  persistConversationFor(slug, conv)
  const history = buildHistoryUntil(conv, group.id)
  await streamAnswer(slug, conv, answer, history)
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
  if (!conv || !props.slug) return
  const slug = props.slug
  const newText = editingText.value.trim()
  if (!newText) return

  // Editing resends the message, which truncates everything after this node —
  // including any answer still generating. Cancel in-flight streams first,
  // otherwise the old `hasStreaming` guard would silently swallow the resend:
  // the 发送 button looked dead whenever a previous turn was still (or got
  // stuck) streaming.
  if (hasStreaming.value) stopAllStreaming()

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
    answers: selectedModels.value.map(model => modelToAnswer(model)),
  }
  if (group.answers[0]) setActiveAnswer(group.id, group.answers[0].id)
  conv.nodes.push(group)
  conv.title = firstUserTitle(conv.nodes) || conv.title
  persistConversationFor(slug, conv)
  await nextTick()
  scrollToBottom(true)

  const history = buildHistoryUntil(conv, group.id)
  await Promise.all(group.answers.map(answer => streamAnswer(slug, conv, answer, history)))
}

function onEditKeydown(e: KeyboardEvent, node: UserNode) {
  if (e.key === 'Escape') { cancelEdit(); return }
  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submitEdit(node) }
}

// Stop only the answers of the conversation on screen — a generation running in
// the background for another paper is not what this button refers to.
function stopAllStreaming() {
  const conv = activeConversation.value
  if (!conv) return
  const answerIds = conv.nodes.flatMap(n =>
    n.role === 'assistantGroup' ? n.answers.filter(a => a.streaming).map(a => a.id) : []
  )
  for (const answerId of answerIds) {
    // Tell the backend to truly cancel the in-flight request (closes the HTTP
    // stream so the provider stops generating / billing).
    const requestId = activeRequestIds.get(answerId)
    if (requestId) invoke('cancel_ai_request', { requestId }).catch(() => {})
    activeRequestIds.delete(answerId)
    for (const suffix of ['', '-reasoning', '-context', '-usage', '-agent', '-confirm', '-confirm-close']) {
      const key = `${answerId}${suffix}`
      const off = unlisteners.get(key)
      if (off) off()
      unlisteners.delete(key)
    }
    const ra = findReactiveAnswer(answerId)
    if (ra?.streaming) {
      ra.streaming = false
      ra.endedAt = performance.now()
      flushStreamRender(ra)
    }
    streamOwners.delete(answerId)
  }
  // Any write the agent was still waiting on is refused when the user stops.
  for (const pending of pendingWrites.value) {
    invoke('resolve_agent_write', { requestId: pending.requestId, approved: false }).catch(() => {})
  }
  pendingWrites.value = []
  persistActiveConversation()
}

// ── Throttled streaming render ────────────────────────────────────────────────
// A streamed answer is re-parsed in full (marked + KaTeX + highlight.js) on every
// refresh, so refreshing per token is O(n²) and freezes the UI. We refresh a
// `displayContent` copy on an interval instead.
//
// The interval adapts, because a fixed one only works for short answers: a 25KB
// answer costs ~150ms per parse, so at a flat 90ms the renderer needs more than a
// second of CPU per second of streaming and never catches up — the whole webview
// (PDF reader included) locks up. We measure what a refresh actually costs on
// this machine and keep re-parsing to roughly a quarter of wall-clock time.
const STREAM_RENDER_MIN_MS = 90
const STREAM_RENDER_MAX_MS = 800
const STREAM_RENDER_DUTY = 4        // interval = measured cost × this
const streamRenderTimers = new Map<string, ReturnType<typeof setTimeout>>()
const streamRenderLast = new Map<string, number>()
const streamRenderCost = new Map<string, number>()

function streamRenderInterval(ansId: string): number {
  const cost = streamRenderCost.get(ansId) ?? 0
  return Math.min(STREAM_RENDER_MAX_MS, Math.max(STREAM_RENDER_MIN_MS, Math.round(cost * STREAM_RENDER_DUTY)))
}

// Publish the new text and time the resulting render. nextTick runs after Vue has
// patched the DOM, so this captures parse + patch — the real cost of a refresh.
function applyStreamRender(ans: AssistantAnswer) {
  const startedAt = performance.now()
  ans.displayContent = ans.content
  nextTick(() => streamRenderCost.set(ans.id, performance.now() - startedAt))
  // An answer streaming in the background must not scroll the paper on screen.
  if (isAnswerVisible(ans.id)) scrollToBottom()
}

function scheduleStreamRender(ans: AssistantAnswer) {
  const now = Date.now()
  const last = streamRenderLast.get(ans.id) ?? 0
  const elapsed = now - last
  const interval = streamRenderInterval(ans.id)
  if (elapsed >= interval) {
    streamRenderLast.set(ans.id, now)
    applyStreamRender(ans)
    return
  }
  if (streamRenderTimers.has(ans.id)) return
  const timer = setTimeout(() => {
    streamRenderTimers.delete(ans.id)
    streamRenderLast.set(ans.id, Date.now())
    const live = findReactiveAnswer(ans.id)
    if (live) applyStreamRender(live)
  }, interval - elapsed)
  streamRenderTimers.set(ans.id, timer)
}

// Final flush so the last tokens are shown even if a throttle window was pending.
function flushStreamRender(ans: AssistantAnswer) {
  const timer = streamRenderTimers.get(ans.id)
  if (timer) { clearTimeout(timer); streamRenderTimers.delete(ans.id) }
  streamRenderLast.delete(ans.id)
  streamRenderCost.delete(ans.id)
  ans.displayContent = ans.content
}

// Clear every pending throttle timer (used on session switch / unmount so
// scheduled renders don't fire against a stale/torn-down conversation).
function clearAllStreamRenderTimers() {
  for (const timer of streamRenderTimers.values()) clearTimeout(timer)
  streamRenderTimers.clear()
  streamRenderLast.clear()
  streamRenderCost.clear()
}

// `slug` is the paper the turn was started on — deliberately a parameter, not
// `props.slug`: the user can switch papers mid-generation and everything below
// must keep targeting that paper and conversation, not what is on screen when
// the tokens arrive.
async function streamAnswer(
  slug: string,
  conv: Conversation,
  answer: AssistantAnswer,
  history: ChatMessage[],
) {
  // Pin the conversation object that owns this answer.
  const owner = activeConversation.value?.id === conv.id ? activeConversation.value : conv
  streamOwners.set(answer.id, { slug, conv: owner })
  const eventName = `paper-ai-chat-${answer.id}`
  const reasoningEventName = `${eventName}-reasoning`
  // Backend cancellation id: sent to `chat_with_library`, used by
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

  // The agent's trail: which tool it reached for, and how that went. This is
  // what replaced the old "context" banner — the model now goes and gets what
  // it needs, so what matters is showing the user what it went and got.
  const unlistenAgent = await listen<{
    phase?: string
    tool?: string
    server?: string | null
    arguments?: unknown
    ok?: boolean
    chars?: number
    preview?: string
    truncated?: boolean
    failed?: string[]
    rounds?: number
    max?: number
  }>(`${eventName}-agent`, (event) => {
    const p = event.payload
    const reactiveAns = findReactiveAnswer(answer.id)
    if (!p || !reactiveAns) return
    if (p.phase === 'tool') {
      if (!reactiveAns.steps) reactiveAns.steps = []
      reactiveAns.steps.push({
        tool: p.tool ?? '',
        server: p.server ?? undefined,
        args: summarizeToolArgs(p.arguments),
        argsJson: JSON.stringify(p.arguments ?? {}, null, 2),
      })
      scrollToBottom()
    } else if (p.phase === 'result') {
      const step = [...(reactiveAns.steps ?? [])].reverse().find(x => x.tool === p.tool && x.ok === undefined)
      if (step) {
        step.ok = p.ok ?? true
        step.chars = p.chars
        step.preview = p.preview
        step.truncated = p.truncated
      }
    } else if (p.phase === 'servers') {
      if (p.failed?.length) reactiveAns.serverErrors = p.failed
    } else if (p.phase === 'limit') {
      reactiveAns.limitHit = { rounds: p.rounds ?? 0, max: p.max ?? 0 }
    }
  })
  unlisteners.set(`${answer.id}-agent`, unlistenAgent)

  // create_paper_note parks in the backend and waits for approval. Show the card
  // above the composer; `-confirm-close` clears it if the backend gave up on its
  // own (timeout, or the generation was stopped) so a stale card never lingers.
  const unlistenConfirm = await listen<{ requestId: string; preview: AgentWritePreview }>(
    `${eventName}-confirm`,
    (event) => {
      const p = event.payload
      if (!p?.requestId || !p.preview) return
      pendingWrites.value = [...pendingWrites.value, { requestId: p.requestId, preview: p.preview }]
    },
  )
  unlisteners.set(`${answer.id}-confirm`, unlistenConfirm)
  const unlistenConfirmClose = await listen<{ requestId: string }>(
    `${eventName}-confirm-close`,
    (event) => { if (event.payload?.requestId) dismissWrite(event.payload.requestId) },
  )
  unlisteners.set(`${answer.id}-confirm-close`, unlistenConfirmClose)

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
    // Agent mode: the model reaches for what it needs instead of being handed a
    // pre-built context. `paperSlug` puts this paper's `get_paper` card in the
    // system prompt, so it starts knowing what it is reading.
    const finalText = await invoke<string>('chat_with_library', {
      messages: history,
      providerId: answer.providerId || null,
      modelId: answer.modelId || null,
      eventName,
      sourcesEventName: `${eventName}-sources`,
      knowledgeSource: 'agent',
      selectedPaperSlugs: [],
      attachments: null,
      useReasoning: useReasoning.value,
      reasoningEffort: useReasoning.value ? effortToSend : null,
      requestId,
      webSearch: false,
      agentMaxRounds: null,
      conversationId: conv.id,
      paperSlug: slug,
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
    const offAgent = unlisteners.get(`${answer.id}-agent`)
    if (offAgent) offAgent()
    unlisteners.delete(`${answer.id}-agent`)
    const offUsage = unlisteners.get(`${answer.id}-usage`)
    if (offUsage) offUsage()
    unlisteners.delete(`${answer.id}-usage`)
    const offConfirm = unlisteners.get(`${answer.id}-confirm`)
    if (offConfirm) offConfirm()
    unlisteners.delete(`${answer.id}-confirm`)
    const offConfirmClose = unlisteners.get(`${answer.id}-confirm-close`)
    if (offConfirmClose) offConfirmClose()
    unlisteners.delete(`${answer.id}-confirm-close`)
    activeRequestIds.delete(answer.id)
    const visible = isAnswerVisible(answer.id)
    streamOwners.delete(answer.id)
    // Save to the paper this answer was started on, even if another one is now
    // on screen.
    persistConversationFor(slug, owner)
    if (visible) scrollToBottom()
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

// Estimated CNY cost for models whose provider doesn't return one (e.g.
// DeepSeek), from the configured prices. The cache-hit split and peak/off-peak
// rules live in utils/modelPricing so this agrees with the usage dashboard.
function estimatedCostCny(answer: AssistantAnswer): number | null {
  if (typeof answer.inputTokens !== 'number' || typeof answer.outputTokens !== 'number') return null
  const provider = ai.settings.providers.find(p => p.id === answer.providerId)
  const model = provider?.models.find(x => x.id === answer.modelId)
  const cost = estimateCostCny(
    model,
    {
      inputTokens: answer.inputTokens,
      outputTokens: answer.outputTokens,
      cacheHitTokens: answer.cacheHitTokens,
      // Priced when the answer was produced, not when it is re-rendered.
      at: answer.createdAt ? new Date(answer.createdAt) : new Date(),
    },
    usdToCnyRate.value,
  )
  return cost != null && cost > 0 ? cost : null
}

function fmtCny(cny: number): string {
  if (cny < 0.01) return '<0.01'
  return cny.toFixed(cny < 1 ? 3 : 2)
}

/**
 * Share of input tokens the provider served from its context cache. Only shown
 * where the number is real: DeepSeek reports it, most providers don't, and a
 * missing count is indistinguishable from a genuine 0% miss.
 */
function cacheHitPercent(answer: AssistantAnswer): number | null {
  if (typeof answer.cacheHitTokens !== 'number') return null
  if (typeof answer.inputTokens !== 'number' || answer.inputTokens <= 0) return null
  // The backend always emits a count, 0 included, so a plain "is it a number"
  // check would paint a meaningless 0% on every provider that has no cache at
  // all. Show it for DeepSeek always (0% there is real information — the cache
  // missed), and elsewhere only when something actually hit.
  const provider = ai.settings.providers.find(p => p.id === answer.providerId)
  const cachingProvider = !!provider?.base_url.toLowerCase().includes('deepseek')
  if (!cachingProvider && answer.cacheHitTokens <= 0) return null
  const pct = (answer.cacheHitTokens / answer.inputTokens) * 100
  return Number.isFinite(pct) ? Math.round(pct) : null
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
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`
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

// ── Composer height ───────────────────────────────────────────────────────────
// The box auto-grows with the text up to 6 lines, but the user can also drag its
// top edge to pick a height outright. The popup window and the sidebar tab are
// very different shapes, so each remembers its own.
const COMPOSER_MIN_H = 44
const COMPOSER_H_KEY = `argus:ai-composer-height${props.standalone ? '-popup' : ''}`

/** Never let the composer eat more than this much of the window. */
function composerMaxH() {
  return Math.max(COMPOSER_MIN_H, Math.round(window.innerHeight * 0.6))
}

const composerHeight = ref<number | null>((() => {
  const n = Number(localStorage.getItem(COMPOSER_H_KEY))
  return Number.isFinite(n) && n >= COMPOSER_MIN_H ? n : null
})())

function persistComposerHeight() {
  try {
    if (composerHeight.value) localStorage.setItem(COMPOSER_H_KEY, String(composerHeight.value))
    else localStorage.removeItem(COMPOSER_H_KEY)
  } catch { /* private mode / quota — the height still applies for this session */ }
}

function resizeTextarea() {
  nextTick(() => {
    const el = textareaEl.value
    if (!el) return
    // A dragged height wins outright: the box stays where the user put it and
    // long input scrolls inside, instead of auto-grow yanking it back. maxHeight
    // has to move with it, or the CSS cap would clip a box dragged taller.
    if (composerHeight.value) {
      const h = Math.min(composerHeight.value, composerMaxH())
      el.style.maxHeight = `${h}px`
      el.style.height = `${h}px`
      return
    }
    el.style.maxHeight = ''
    const lineH = parseFloat(getComputedStyle(el).lineHeight) || 19
    const padTop = parseFloat(getComputedStyle(el).paddingTop) || 10
    el.style.height = 'auto'
    const lines = Math.ceil((el.scrollHeight - padTop) / lineH)
    const maxLines = 6
    const snapped = Math.min(maxLines, lines) * lineH + padTop
    el.style.height = `${snapped}px`
  })
}

watch(composerHeight, resizeTextarea)

function onComposerResizeStart(e: PointerEvent) {
  const el = textareaEl.value
  if (!el) return
  const handle = e.currentTarget as HTMLElement
  const startY = e.clientY
  const startH = el.offsetHeight
  handle.setPointerCapture?.(e.pointerId)
  // Dragging upward sweeps over the message list, which is selectable text.
  beginDragSelectionGuard()

  const onMove = (ev: PointerEvent) => {
    // The composer is pinned to the bottom, so dragging UP grows it — hence the
    // inverted delta.
    composerHeight.value = Math.min(
      composerMaxH(),
      Math.max(COMPOSER_MIN_H, startH + startY - ev.clientY),
    )
  }
  const onUp = (ev: PointerEvent) => {
    endDragSelectionGuard()
    handle.releasePointerCapture?.(ev.pointerId)
    handle.removeEventListener('pointermove', onMove)
    handle.removeEventListener('pointerup', onUp)
    handle.removeEventListener('pointercancel', onUp)
    persistComposerHeight()
  }
  handle.addEventListener('pointermove', onMove)
  handle.addEventListener('pointerup', onUp)
  handle.addEventListener('pointercancel', onUp)
}

/** Double-click the grabber to hand the height back to auto-grow. */
function resetComposerHeight() {
  composerHeight.value = null
  persistComposerHeight()
  resizeTextarea()
}

function handleKeydown(e: KeyboardEvent) {
  // `isComposing` / keyCode 229 reliably detect an active IME composition,
  // avoiding the race-prone Date.now() heuristic that could send half-typed text.
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
    e.preventDefault()
    sendMessage()
  }
}

// ── Follow-the-stream scrolling ───────────────────────────────────────────────
// Whether new streamed text should pull the view down. This is tracked from the
// user's own scrolling rather than re-measured at render time: an answer that is
// still streaming changes the container's height constantly, so a distance-from-
// bottom check taken right after a DOM patch can read a height that is about to
// change again and yank the user back down. Once they scroll up they stay put
// until they come back to the bottom themselves.
const stickToBottom = ref(true)
const STICK_THRESHOLD_PX = 120

function onMessagesScroll() {
  const el = messagesEl.value
  if (!el) return
  stickToBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < STICK_THRESHOLD_PX
}

/** Scrolling up is an explicit "let me read" — release immediately, don't wait
 *  for the scroll event, which a mid-stream height change could contradict. */
function onMessagesWheel(e: WheelEvent) {
  if (e.deltaY < 0) stickToBottom.value = false
}

/** One tool call's arguments, shortened for the trail line. Mirrors the library
 *  chat's summary so the two panels read the same. */
function summarizeToolArgs(args: unknown): string {
  if (!args || typeof args !== 'object') return ''
  return Object.entries(args as Record<string, unknown>)
    .filter(([, v]) => v !== null && v !== undefined && v !== '')
    .map(([k, v]) => {
      const text = typeof v === 'string' ? v : JSON.stringify(v)
      return `${k}: ${text.length > 40 ? text.slice(0, 40) + '…' : text}`
    })
    .join(', ')
}

function scrollToBottom(force = false) {
  if (force) stickToBottom.value = true
  nextTick(() => {
    const el = messagesEl.value
    if (!el) return
    if (!stickToBottom.value) return
    el.scrollTop = el.scrollHeight
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
  const visible = isAnswerVisible(answerId)
  const owner = streamOwners.get(answerId)
  streamOwners.delete(answerId)
  if (owner) persistConversationFor(owner.slug, owner.conv)
  else persistActiveConversation()
  if (visible) scrollToBottom()
}

function closeFloating(e: MouseEvent) {
  if (modelMenuRoot.value && !modelMenuRoot.value.contains(e.target as Node)) {
    showModelMenu.value = false
  }
  if (reasoningRoot.value && !reasoningRoot.value.contains(e.target as Node)) {
    reasoningOpen.value = false
  }
}

// The paper this view is currently showing, tracked separately from `props.slug`
// so the watcher knows which paper it is leaving.
let sessionSlug: string | null = null
// Increments on every switch; a slow, superseded run must not clear the guard
// (or write state) belonging to a newer one.
let slugRunToken = 0

watch(() => props.slug, async (slug) => {
  // Snapshot the paper being left so switching back restores the same
  // conversation. Answers still generating keep streaming into that object —
  // `streamOwners` holds it, so clearing the refs below doesn't interrupt them.
  if (sessionSlug && sessionSlug !== slug) rememberSession(sessionSlug)
  sessionSlug = slug
  // Stream listeners are deliberately NOT torn down here: an answer that is
  // still generating must keep running in the background. Each stream removes
  // its own listeners when it finishes.
  showHistory.value = false
  showModelMenu.value = false
  activeConversation.value = null
  conversations.value = []
  activeAnswerTabs.value = {}
  input.value = ''
  abstractAvailable.value = false
  if (slug) {
    await refreshAbstractAvailability(slug)
    if (props.slug !== slug) return
    await loadConversations(slug)
  }
}, { immediate: true })

watch(() => allSelectableModels.value.map(modelKey).join('|'), ensureDefaultModels, { immediate: true })
watch(input, resizeTextarea)
watch(activeConversationIsMetadataExtraction, (isMetadataExtraction) => {
  if (isMetadataExtraction) showModelMenu.value = false
})

onMounted(async () => {
  // Apply a remembered composer height right away — otherwise it wouldn't take
  // effect until the first keystroke re-ran the sizing.
  if (composerHeight.value) resizeTextarea()
  await settingsStore.load()
  if (!ai.loaded) await ai.load()
  ensureDefaultModels()
  if (typeof ResizeObserver !== 'undefined' && rootRef.value) {
    panelWidth.value = rootRef.value.offsetWidth
    panelResizeObserver = new ResizeObserver(entries => {
      panelWidth.value = entries[0]?.contentRect.width ?? 0
    })
    panelResizeObserver.observe(rootRef.value)
  }
  document.addEventListener('mousedown', closeFloating)
  window.addEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  unlistenKeepalive = await listen<KeepaliveStatus>('cache-keepalive', (event) => {
    keepalive.value = event.payload ?? { active: false }
  })
  keepaliveClock = setInterval(() => { nowMs.value = Date.now() }, 60_000)

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
    // Pin the extraction to this paper so it survives a paper switch too.
    streamOwners.set(answer_id, { slug: ev.payload.slug, conv: activeConversation.value! })

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
  panelResizeObserver?.disconnect()
  panelResizeObserver = null
  document.removeEventListener('mousedown', closeFloating)
  window.removeEventListener('argus-paper-meta-updated', onPaperMetaUpdated)
  // Only stop the keepalive if it is one of *these* conversations being held:
  // the library chat and the canvas chat share the same single slot, and
  // closing this panel must not cancel the warm prefix they are paying for.
  if (keepalive.value.active && conversations.value.some(c => c.id === keepalive.value.conversationId)) {
    invoke('disarm_cache_keepalive').catch(() => {})
  }
  unlistenKeepalive?.()
  if (keepaliveClock) clearInterval(keepaliveClock)
  unlistenMetaStart?.()
  unlistenMetaDone?.()
  unlistenMetaError?.()
  for (const off of unlisteners.values()) off()
  unlisteners.clear()
  // Refuse any write the agent was still waiting on so the backend doesn't hang.
  for (const pending of pendingWrites.value) {
    invoke('resolve_agent_write', { requestId: pending.requestId, approved: false }).catch(() => {})
  }
  pendingWrites.value = []
  clearAllStreamRenderTimers()
})

</script>

<template>
  <div ref="rootRef" class="paper-ai" :class="{ standalone: props.standalone }">
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
            <span class="history-title">
              <span v-if="isCacheWarm(conv.id)" class="conv-cache-dot" :title="keepaliveTitle" />
              {{ conv.title }}
            </span>
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
                  <span class="model-name">
                    {{ model.displayName }}
                    <span
                      v-if="offerOf(model)"
                      class="offer-tag"
                      :class="[offerOf(model)!.kind, { idle: !offerOf(model)!.activeNow }]"
                      :title="offerOf(model)!.title"
                    >{{ offerOf(model)!.label }}</span>
                  </span>
                  <span class="model-meta">
                    <span
                      class="row-size"
                      :class="{ assumed: !sizeOf(model).known }"
                      :title="sizeOf(model).title"
                    >{{ sizeOf(model).text }}</span>
                    {{ capabilitiesLabel(model).join(' · ') || model.modelId }}
                  </span>
                </span>
              </button>
            </div>
            <div class="menu-foot">点击切换模型。</div>
          </div>
        </div>

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
        <div
          ref="messagesEl"
          class="messages"
          :class="{ 'messages--no-floating-model': activeConversationIsMetadataExtraction }"
          @click="onMsgContainerClick"
          @scroll.passive="onMessagesScroll"
          @wheel.passive="onMessagesWheel"
        >
        <div v-if="!activeConversation?.nodes.length" class="empty-chat">
          <!-- A hand-drawn reader rather than a glyph: this panel is someone
               sitting down with a paper, and the drawing says that faster than
               a sparkle does. -->
          <Icon class="empty-doodle" icon="doodle:reading-document" width="72" height="72" />
          <p>基于这篇论文开始新对话</p>
          <span>问它任何问题，它会自己去翻这篇论文。</span>
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
                    <span v-if="answer.streaming" class="live-dot" />
                  </div>
                </div>

                <!-- What the agent went and read to answer. Replaces the old
                     "context sent" banner: the model fetches what it needs now,
                     so what is worth showing is what it actually fetched. -->
                <div v-if="answer.steps?.length" class="agent-trail">
                  <div class="agent-trail-head" :class="{ busy: agentRunning(answer) }">
                    <Icon icon="fluent:bot-sparkle-24-regular" width="12" height="12" />
                    <span v-if="agentRunning(answer)">正在查资料… 已调用 {{ answer.steps.length }} 次工具</span>
                    <span v-else>调用了 {{ answer.steps.length }} 次工具</span>
                  </div>
                  <div v-for="(step, i) in answer.steps" :key="i" class="agent-step-wrap">
                    <button
                      class="agent-step"
                      :class="{ open: expandedSteps.has(stepKey(answer.id, i)) }"
                      @click="toggleStep(answer.id, i)"
                    >
                      <Icon
                        class="agent-step-chevron"
                        :class="{ open: expandedSteps.has(stepKey(answer.id, i)) }"
                        icon="fluent:chevron-right-24-regular"
                        width="10"
                        height="10"
                      />
                      <Icon
                        :icon="step.ok === false
                          ? 'fluent:dismiss-circle-24-regular'
                          : step.ok === undefined
                            ? 'fluent:arrow-clockwise-24-regular'
                            : 'fluent:checkmark-circle-24-regular'"
                        width="11"
                        height="11"
                        :class="{ spin: step.ok === undefined, failed: step.ok === false }"
                      />
                      <span v-if="step.server" class="agent-step-server" :title="`来自 MCP 服务器：${step.server}`">{{ step.server }}</span>
                      <code class="agent-step-tool">{{ step.tool }}</code>
                      <span v-if="step.args" class="agent-step-args">{{ step.args }}</span>
                      <span v-if="step.chars" class="agent-step-size">{{ formatChars(step.chars) }}</span>
                    </button>

                    <!-- What the model actually sent and got back. Collapsed by
                         default; this is for checking an answer, not reading. -->
                    <div v-if="expandedSteps.has(stepKey(answer.id, i))" class="agent-step-detail">
                      <div class="agent-detail-label">参数</div>
                      <pre class="agent-detail-code">{{ step.argsJson }}</pre>
                      <div class="agent-detail-label">
                        返回
                        <span v-if="step.chars" class="agent-detail-note">
                          （{{ formatChars(step.chars) }} 字符<template v-if="step.truncated">，已超出模型上下文预算并被截断</template>）
                        </span>
                      </div>
                      <pre v-if="step.preview" class="agent-detail-code">{{ step.preview }}</pre>
                      <div v-else class="agent-detail-note">这一步没有返回内容</div>
                    </div>
                  </div>
                  <div v-if="answer.limitHit" class="agent-note">
                    工具调用达到上限（{{ answer.limitHit.rounds }}/{{ answer.limitHit.max }} 轮），下面是它用已有信息写的回答。
                  </div>
                  <div v-if="answer.serverErrors?.length" class="agent-note">
                    这些 MCP 服务器没能启动：{{ answer.serverErrors.join('、') }}
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
                    <MarkdownBody
                      v-if="answer.content"
                      :content="answer.displayContent ?? answer.content"
                      :streaming="true"
                    />
                    <div v-else-if="!answer.reasoningContent" class="thinking-placeholder">{{ answer.withReasoning ? '正在思考…' : '生成中…' }}</div>
                  </template>
                  <MarkdownBody v-else :content="answer.content" />
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
                  <span
                    v-if="cacheHitPercent(answer) !== null"
                    class="usage-cache"
                    :title="`命中上下文缓存 ${answer.cacheHitTokens} / ${answer.inputTokens} 输入 tokens，按缓存价计费`"
                  >
                    <Icon icon="fluent:database-24-regular" width="10" height="10" />
                    {{ cacheHitPercent(answer) }}%
                  </span>
                  <span v-if="showAnswerSpeed && answerSpeed(answer)" class="msg-speed">{{ answerSpeed(answer) }}</span>
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

        <!-- Approval for a write the agent asked for, directly above the composer. -->
        <div v-if="currentWrite" class="write-confirm-slot">
          <WriteConfirmCard
            :key="currentWrite.requestId"
            :preview="currentWrite.preview"
            :queued="pendingWrites.length - 1"
            @approve="answerWrite(true)"
            @reject="answerWrite(false)"
          />
        </div>

        <footer class="composer">
        <div
          class="composer-resizer"
          title="拖动调整输入框高度（双击恢复自适应）"
          @pointerdown="onComposerResizeStart"
          @dblclick="resetComposerHeight"
        />
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
                <span class="history-title">
                  <span v-if="isCacheWarm(conv.id)" class="conv-cache-dot" :title="keepaliveTitle" />
                  {{ conv.title }}
                </span>
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
/* Parameter count. Plain text rather than a filled chip: every row carries one,
   and a grid of grey blocks buried the model names they were meant to annotate.
   The dot separates it from the capability list that follows. */
.row-size {
  margin-right: 5px;
  font-size: 10px;
  font-weight: 600;
  color: var(--text-tertiary);
}
.row-size::after {
  content: '·';
  margin-left: 5px;
  font-weight: 400;
  opacity: 0.55;
}
.row-size.assumed { opacity: 0.6; font-weight: 500; }

/* Price-list badge. A discount outside its window is dimmed rather than hidden,
   so the model still reads as "cheaper at some hours" without claiming it is
   cheaper right now. */
.offer-tag {
  display: inline-block;
  margin-left: 6px;
  padding: 0 5px;
  border-radius: var(--radius-sm);
  font-size: 9.5px;
  font-weight: 600;
  line-height: 15px;
  vertical-align: middle;
}
.offer-tag.free { color: #15803d; background: color-mix(in srgb, #22c55e 16%, transparent); }
.offer-tag.discount { color: #b45309; background: color-mix(in srgb, #f59e0b 18%, transparent); }
.offer-tag.discount.idle { color: var(--text-tertiary); background: color-mix(in srgb, var(--text-tertiary) 12%, transparent); }

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
.empty-doodle {
  color: color-mix(in srgb, var(--accent) 62%, var(--text-tertiary));
  margin-bottom: 2px;
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
  /* Breathing, not a one-shot ripple: a model can still be answering while a
     different variant is on screen, so this has to read as "ongoing" at a
     glance. Halo and icon share the timing so they pulse as one. */
  animation: variant-halo 1.6s ease-in-out infinite;
  border-color: color-mix(in srgb, var(--accent) 45%, transparent);
}
.variant-btn.streaming .variant-logo,
.variant-btn.streaming .variant-letter {
  animation: variant-breathe 1.6s ease-in-out infinite;
}
@keyframes variant-halo {
  0%, 100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 38%, transparent); }
  50% { box-shadow: 0 0 0 5px color-mix(in srgb, var(--accent) 0%, transparent); }
}
@keyframes variant-breathe {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}
@media (prefers-reduced-motion: reduce) {
  .variant-btn.streaming,
  .variant-btn.streaming .variant-logo,
  .variant-btn.streaming .variant-letter {
    animation: none;
  }
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
.usage-cache {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}
.usage-cost {
  color: var(--text-secondary);
  font-weight: 500;
  margin-left: 2px;
}
.usage-cost-est { color: var(--text-tertiary); font-weight: 400; }
.error-badge { color: #ef4444; }

/* Holds the agent write-approval card, aligned to the composer's inner width. */
.write-confirm-slot { flex-shrink: 0; padding: 0 10px; }

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
/* Drag the composer's top edge to set its height; the grip only shows on hover
   so it stays out of the way. Sits above the context bar's buttons. */
.composer-resizer {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 8px;
  z-index: 3;
  cursor: ns-resize;
  touch-action: none;
  user-select: none;
  -webkit-user-select: none;
}
.composer-resizer::after {
  content: '';
  position: absolute;
  top: 2px;
  left: 50%;
  transform: translateX(-50%);
  width: 38px;
  height: 3px;
  border-radius: 2px;
  background: var(--border-default);
  opacity: 0;
  transition: opacity .15s ease;
}
.composer-resizer:hover::after { opacity: 1; }
.standalone .composer-box {
  padding: 12px 12px 8px 14px;
  background: #f5f5f7;
  border-color: rgba(0, 0, 0, 0.09);
}
.standalone .composer-box:focus-within {
  border-color: rgba(34, 63, 121, 0.35);
  background: #fafafa;
}
/* Matches the sidebar's resting height (~3 lines). This used to be 24px — one
   line — which read as cramped in a window this size. Line spacing is kept in
   step with the sidebar too; 1.55 was noticeably airier at the same font size. */
.standalone .composer-input {
  min-height: 68px;
  max-height: 240px;
  padding: 0;
  font-size: 14px;
  line-height: 1.45;
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
  gap: 2px;
  padding: 5px 7px 7px;
}
.toolbar-spacer { flex: 1; }
/* Most of the space between these icons was the buttons' own padding, not the
   gap: a 15px glyph in a 30px box carries 7.5px of air on each side, so even at
   gap:1 the glyphs sat ~16px apart. Shrinking the box is what actually tightens
   the run — 24px keeps a comfortable click target and lands the glyphs ~11px
   apart. Keep this in step with LibraryChat's composer. */
.toolbar-btn {
  position: relative;
  width: 24px;
  height: 24px;
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
.agent-trail {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-bottom: 8px;
  padding: 7px 9px;
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
}
.agent-trail-head {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-bottom: 3px;
  font-size: 10.5px;
  font-weight: 600;
  color: var(--text-tertiary);
}
.agent-trail-head svg { flex-shrink: 0; }
.agent-trail-head.busy { color: var(--accent); }

.agent-step-wrap { display: flex; flex-direction: column; }
.agent-step {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  min-width: 0;
  padding: 2px 4px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  font-size: 11px;
  text-align: left;
  cursor: pointer;
}
.agent-step:hover { background: var(--bg-hover); }
.agent-step.open { background: var(--bg-hover); }
.agent-step svg { flex-shrink: 0; color: var(--text-tertiary); }
.agent-step svg.failed { color: #ef4444; }
.agent-step-chevron { transition: transform 0.14s ease; }
.agent-step-chevron.open { transform: rotate(90deg); }
.agent-step-size { flex-shrink: 0; margin-left: auto; color: var(--text-tertiary); font-variant-numeric: tabular-nums; }
.agent-step-tool {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-secondary);
  background: none;
  padding: 0;
}

/* The payload behind a row. Capped and scrollable: a full-text slice is tens of
   thousands of characters, and it must not push the answer off the screen. */
.agent-step-detail {
  margin: 2px 0 6px 20px;
  padding: 7px 9px;
  border-left: 2px solid var(--border-default);
  border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
  background: var(--bg-primary);
}
.agent-detail-label {
  font-size: 10px;
  font-weight: 600;
  color: var(--text-tertiary);
  margin-bottom: 3px;
}
.agent-detail-label + .agent-detail-code { margin-bottom: 8px; }
.agent-detail-note { font-weight: 400; color: var(--text-tertiary); }
.agent-detail-code {
  margin: 0;
  max-height: 220px;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: 10.5px;
  line-height: 1.55;
  color: var(--text-secondary);
  white-space: pre-wrap;
  word-break: break-word;
}

/* Marks the one conversation whose prompt cache is being kept warm. */
.conv-cache-dot {
  display: inline-block;
  width: 6px;
  height: 6px;
  margin-right: 5px;
  border-radius: 50%;
  background: #22c55e;
  vertical-align: middle;
  flex-shrink: 0;
  animation: conv-cache-breathe 2.4s ease-in-out infinite;
}
@keyframes conv-cache-breathe {
  0%, 100% { opacity: 1; box-shadow: 0 0 0 0 rgba(34, 197, 94, 0.45); }
  50% { opacity: 0.4; box-shadow: 0 0 0 4px rgba(34, 197, 94, 0); }
}
.agent-step-server {
  flex-shrink: 0;
  padding: 0 5px;
  border-radius: 4px;
  font-size: 9.5px;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
}
.agent-step-args {
  min-width: 0;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.agent-note { font-size: 10.5px; line-height: 1.5; color: #b45309; }
.spin { animation: agent-spin 1.1s linear infinite; }
@keyframes agent-spin { to { transform: rotate(360deg); } }

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
