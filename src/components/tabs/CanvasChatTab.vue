<script setup lang="ts">
// AI chat scoped to one canvas.
//
// The model answers in agent mode, so it reads the canvas itself through the
// library tools (`get_canvas` returns every node with its paper slug, plus the
// edges and their labels) and can follow any node into the paper behind it. The
// only thing this component tells it up front is which canvas is on screen —
// deliberately not a snapshot of the nodes, because the user edits the canvas
// while asking about it and a snapshot would go stale mid-conversation.
//
// Conversations are stored beside the canvas (canvases/<id>.chat.json), the way
// a paper's chat lives in the paper's folder: open the canvas and its history is
// there, delete the canvas and the history goes with it.
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useAiStore, type ModelOption } from '../../stores/ai'
import { useCanvasStore } from '../../stores/canvas'
import ChatAnswerBody from '../ChatAnswerBody.vue'
import ContextMeter from '../ContextMeter.vue'
import { beginChatRun, applyRunUsage, legacyChatRun, recordCompactionUsage, type ChatRun, type StreamUsagePayload } from '../../utils/chatRunStats'
import { applyContextCheckpoint, historyFingerprint, withContextDraft, type ContextCheckpoint, type ContextUsage, type PreparedContext } from '../../utils/chatContext'
import { buildToolExchangeMessages } from '../../utils/agentHistory'
import { SEND_PAPER_IMAGE_TOOL, type AgentStepImage } from '../../types/chatImages'
import WriteConfirmCard from '../WriteConfirmCard.vue'
import CanvasEditConfirmCard from '../CanvasEditConfirmCard.vue'
import ProviderBalanceTag from '../ProviderBalanceTag.vue'
import ServerToolTraceCard from '../ServerToolTraceCard.vue'
import { mergeServerToolTrace } from '../../utils/serverToolTrace'
import { modelLogo as logoFor, modelCapabilityText } from '../../utils/modelLogo'
import { modelOffer, modelSizeLabel } from '../../utils/modelOffers'
import type {
  AgentWritePreview, CanvasEditPreview, ChatMessage, ModelSelection, ServerToolTrace,
} from '../../types'

const props = defineProps<{ canvasId: string }>()

const { t } = useI18n()
const ai = useAiStore()
const canvasStore = useCanvasStore()

interface AgentStep {
  tool: string
  args: string
  argsJson?: string
  ok?: boolean
  preview?: string
  chars?: number
  truncated?: boolean
  images?: AgentStepImage[]
}

interface CanvasMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  /** Transient throttled copy of `content` used while streaming. Repainting on
   *  every token re-renders the whole panel and forces a layout of an answer
   *  that keeps growing, which is what froze the tab on long answers. */
  displayContent?: string
  createdAt: string
  /** Transient: true while this answer is still streaming. Never persisted. */
  streaming?: boolean
  error?: boolean
  steps?: AgentStep[]
  /** Transient: what OpenRouter's server tools contributed to this answer. */
  serverTools?: ServerToolTrace
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  cacheHitTokens?: number
}

interface CanvasConversation {
  id: string
  title: string
  messages: CanvasMessage[]
  createdAt: string
  updatedAt: string
  contextStates?: Record<string, { checkpoint?: ContextCheckpoint; usage?: ContextUsage }>
  usageRuns?: Record<string, ChatRun>
}

function legacyCanvasRuns(conv: CanvasConversation): ChatRun[] {
  return conv.messages.filter(m => m.role === 'assistant' && typeof m.inputTokens === 'number')
    .map(answer => legacyChatRun(answer, '', '', answer.steps?.length))
}
const conversations = ref<CanvasConversation[]>([])
const activeConvId = ref<string | null>(null)
const input = ref('')
const loading = ref(false)
const historyOpen = ref(false)
const model = ref<ModelSelection | null>(null)
const messagesEl = ref<HTMLElement | null>(null)
const errorText = ref('')
const previewImage = ref<string | null>(null)
const contextCompacting = ref(false)
const contextError = ref('')
const contextNotice = ref('')
const libraryRoot = ref<string | null>(null)
let canvasEpoch = 0
let libraryRevision = 0
let disposed = false
let offLibrary: UnlistenFn | null = null
let contextInspection = 0
let contextInspectTimer: ReturnType<typeof setTimeout> | null = null
const stoppedRequests = new Set<string>()

// A write the agent asked for, waiting on the user. Two shapes share the queue:
// a note (the same card the library chat uses) and a canvas edit (drawn on the
// canvas, approved here). Nothing happens until the card is answered.
type PendingWrite =
  | { kind: 'note'; requestId: string; preview: AgentWritePreview }
  | { kind: 'canvas'; requestId: string; preview: CanvasEditPreview }
const pendingWrites = ref<PendingWrite[]>([])
const currentWrite = computed<PendingWrite | null>(() => pendingWrites.value[0] ?? null)
// Narrowed views so each card gets the exact preview type it expects.
const currentCanvasWrite = computed(() =>
  currentWrite.value?.kind === 'canvas' ? currentWrite.value : null)
const currentNoteWrite = computed(() =>
  currentWrite.value?.kind === 'note' ? currentWrite.value : null)

// ── Model picker ─────────────────────────────────────────────────────────────
//
// A floating pill over the top of the thread, the way the paper AI tab and the
// library chat do it, rather than a select buried in the composer.
const modelMenuOpen = ref(false)
const modelPickerRoot = ref<HTMLElement | null>(null)

const selectedOption = computed(() => ai.findModel(model.value))
const selectedLabel = computed(() =>
  selectedOption.value?.displayName ?? model.value?.modelId ?? t('canvasChat.pickModel'))

function providerKindOf(option?: ModelOption | null) {
  return ai.settings.providers.find(p => p.id === option?.providerId)?.kind
}
function logoOf(option?: ModelOption | null) {
  return logoFor(option, providerKindOf(option))
}
function sizeOf(option: ModelOption) {
  return modelSizeLabel(option.paramBillions)
}
function offerOf(option: ModelOption) {
  return modelOffer({
    is_free: option.isFree,
    discount_percent: option.discountPercent,
    discount_windows: option.discountWindows,
  })
}
function isSelected(option: ModelOption) {
  return model.value?.providerId === option.providerId && model.value?.modelId === option.modelId
}
function pickModel(option: ModelOption) {
  model.value = { providerId: option.providerId, modelId: option.modelId }
  modelMenuOpen.value = false
}
/** Balances are looked up when the picker opens, not on every keystroke. */
function toggleModelMenu() {
  modelMenuOpen.value = !modelMenuOpen.value
  if (modelMenuOpen.value) void ai.loadBalances()
}
function closeModelMenu(e: MouseEvent) {
  if (!modelPickerRoot.value?.contains(e.target as Node)) modelMenuOpen.value = false
}

const activeConv = computed(() =>
  conversations.value.find(c => c.id === activeConvId.value) ?? null)
const messages = computed(() => activeConv.value?.messages ?? [])
const canvasName = computed(() => canvasStore.canvasById(props.canvasId)?.name ?? '')
const canSend = computed(() => !!input.value.trim() && !loading.value && !!model.value)

// Listeners and the cancel id of the answer in flight, so switching canvas or
// unmounting stops cleanly.
let unlisteners: UnlistenFn[] = []
let activeRequestId: string | null = null
let saveTimer: ReturnType<typeof setTimeout> | null = null

function newId() {
  return crypto.randomUUID()
}

function blankConversation(): CanvasConversation {
  const now = new Date().toISOString()
  return { id: newId(), title: '', messages: [], createdAt: now, updatedAt: now }
}

/** The context block sent ahead of every question in this canvas.
 *
 *  Byte-identical across turns (it names the canvas, it does not describe it),
 *  which is what lets the provider's prompt cache hit on the follow-up. The
 *  model reads the actual contents with `get_canvas`, so it always sees the
 *  canvas as it is right now rather than as it was when the chat started. */
function canvasContext(): ChatMessage {
  return {
    role: 'system',
    content:
      `The user is looking at a canvas in their library: "${canvasName.value}" ` +
      `(canvas_id: ${props.canvasId}).\n\n` +
      'Unless they say otherwise, questions are about this canvas. Call `get_canvas` with that ' +
      'id to read it — you get every node (paper nodes carry a `slug`, annotation nodes carry ' +
      "the user's own text) and every edge with its label. Follow a node into `get_paper`, " +
      '`get_paper_fulltext`, `get_note` or `get_highlights` when the answer needs what is ' +
      'inside a paper. Re-read the canvas rather than relying on an earlier turn: the user ' +
      'edits it while you talk.',
  }
}

// ── Persistence ──────────────────────────────────────────────────────────────

/** Strip transient fields so a reload does not resurrect a "streaming" answer. */
function persistable(list: CanvasConversation[]) {
  return list
    .filter(c => c.messages.length > 0)
    .map(c => ({
      ...c,
      messages: c.messages.map(({ streaming: _streaming, displayContent: _display, ...m }) => ({
        ...m,
        steps: m.steps?.map(step => ({
          ...step,
          images: step.images?.map(({ dataUrl: _liveOnly, ...image }) => image),
        })),
      })),
    }))
}

function scheduleSave() {
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => { void save() }, 400)
}

// The backend merges canvas conversations across machines (union by id), so a
// delete has to be expressed as a tombstone or it just resurrects from disk.
// Diff the ids we've seen against what we're saving to spot real deletes.
const knownConvIds = new Map<string, Set<string>>()

function deletionTombstones(canvasId: string, persisted: { id: string }[]): { id: string; deleted_at: string }[] {
  const known = knownConvIds.get(canvasId) ?? new Set<string>()
  const currentIds = new Set(persisted.map(c => c.id))
  const tombs = [...known].filter(id => !currentIds.has(id)).map(id => ({ id, deleted_at: new Date().toISOString() }))
  currentIds.forEach(id => known.add(id))
  knownConvIds.set(canvasId, known)
  return tombs
}

async function save() {
  saveTimer = null
  const canvasId = props.canvasId
  try {
    const persisted = persistable(conversations.value)
    await invoke('save_canvas_ai_conversations', {
      canvasId,
      conversations: persisted,
      tombstones: deletionTombstones(canvasId, persisted),
      expectedRoot: libraryRoot.value,
    })
  } catch (e) {
    console.error('save_canvas_ai_conversations:', e)
  }
}

async function load(canvasId: string) {
  const epoch = canvasEpoch
  conversations.value = []
  activeConvId.value = null
  errorText.value = ''
  try {
    const raw = await invoke<unknown>('get_canvas_ai_conversations', { canvasId })
    if (disposed || epoch !== canvasEpoch || canvasId !== props.canvasId) return
    if (Array.isArray(raw)) {
      conversations.value = raw
        .filter((c): c is CanvasConversation => !!c && typeof c === 'object' && Array.isArray((c as CanvasConversation).messages))
        .map(c => ({ ...c, messages: c.messages.map(m => ({ ...m, streaming: false })) }))
        .sort((a, b) => (b.updatedAt ?? '').localeCompare(a.updatedAt ?? ''))
      knownConvIds.set(canvasId, new Set(conversations.value.map(c => c.id)))
    }
  } catch (e) {
    console.error('get_canvas_ai_conversations:', e)
  }
  // Canvas changed under us while the read was in flight.
  if (disposed || epoch !== canvasEpoch || canvasId !== props.canvasId) return
  const conv = conversations.value[0] ?? blankConversation()
  if (!conversations.value.includes(conv)) conversations.value.unshift(conv)
  activeConvId.value = conv.id
}

// ── Sending ──────────────────────────────────────────────────────────────────

function contextModelKey(sel: ModelSelection) { return `${sel.providerId}::${sel.modelId}` }
function rawContextHistory(records: CanvasMessage[], conversationId?: string): ChatMessage[] {
  return records.filter(m => !m.streaming).flatMap(m => {
    if (m.role === 'user') return [{ role: m.role, content: m.content }]
    const result = buildToolExchangeMessages(m.steps, m.id, conversationId)
    if (m.content.trim()) result.push({ role: 'assistant', content: m.content })
    return result
  })
}
function canvasHistory(conv: CanvasConversation, sel: ModelSelection, records = conv.messages): ChatMessage[] {
  return [canvasContext(), ...applyContextCheckpoint(records, conv.contextStates?.[contextModelKey(sel)]?.checkpoint, records => rawContextHistory(records, conv.id))]
}
function setContextUsage(conv: CanvasConversation, sel: ModelSelection, usage: ContextUsage) {
  const states = conv.contextStates ??= {}
  const state = states[contextModelKey(sel)] ??= {}
  state.usage = usage
}
function setContextCheckpoint(conv: CanvasConversation, sel: ModelSelection, bound: string, messages: ChatMessage[]) {
  const index = conv.messages.findIndex(m => m.id === bound)
  if (index < 0) return
  const states = conv.contextStates ??= {}
  const state = states[contextModelKey(sel)] ??= {}
  state.checkpoint = { throughMessageId: bound, messages, fingerprint: historyFingerprint(rawContextHistory(conv.messages.slice(0, index + 1), conv.id)), createdAt: new Date().toISOString() }
}
function contextArguments(conv: CanvasConversation, sel: ModelSelection, messages: ChatMessage[]) {
  return { messages, providerId: sel.providerId, modelId: sel.modelId, canvasId: props.canvasId, plainFallback: false,
    selectedPaperSlugs: [], webSearch: false, expectedRoot: libraryRoot.value,
    previousUsage: conv.contextStates?.[contextModelKey(sel)]?.usage ?? null }
}
const contextDisplay = computed(() => {
  const sel = model.value
  const usage = sel && activeConv.value?.contextStates?.[contextModelKey(sel)]?.usage
  if (!usage) return null
  return withContextDraft(usage, input.value)
})
async function inspectContext() {
  const conv = activeConv.value, sel = model.value, root = libraryRoot.value
  if (!conv || !sel || !root || loading.value || disposed) return
  const revision = ++contextInspection, epoch = canvasEpoch, fingerprint = historyFingerprint(rawContextHistory(conv.messages, conv.id))
  try {
    const result = await invoke<PreparedContext>('prepare_library_chat_context', {
      ...contextArguments(conv, sel, canvasHistory(conv, sel)), mode: 'inspect', eventName: `canvas-context-${conv.id}`, requestId: null,
    })
    if (result?.usage && revision === contextInspection && epoch === canvasEpoch && !disposed && !loading.value
      && activeConv.value === conv && model.value && contextModelKey(model.value) === contextModelKey(sel)
      && fingerprint === historyFingerprint(rawContextHistory(conv.messages, conv.id))) setContextUsage(conv, sel, result.usage)
  } catch { /* No configured model/library yet. */ }
}
function scheduleContextInspection() {
  contextInspection++
  contextError.value = ''; contextNotice.value = ''
  if (contextInspectTimer) clearTimeout(contextInspectTimer)
  contextInspectTimer = setTimeout(() => { contextInspectTimer = null; void inspectContext() }, 350)
}
watch(() => [props.canvasId, activeConvId.value, activeConv.value?.updatedAt, model.value?.providerId, model.value?.modelId, libraryRoot.value, loading.value], scheduleContextInspection)

async function compactContext() {
  const conv = activeConv.value, sel = model.value
  if (!conv || !sel || loading.value || !libraryRoot.value || !conv.messages.length) return
  const epoch = canvasEpoch, requestId = newId(), bound = conv.messages[conv.messages.length - 1].id
  const fingerprint = historyFingerprint(rawContextHistory(conv.messages, conv.id))
  activeRequestId = requestId; loading.value = true; contextCompacting.value = true
  contextError.value = ''; contextNotice.value = ''
  const offUsage = await listen<StreamUsagePayload & { id: string }>(`canvas-context-${requestId}-compaction-usage`, e => {
    if (!disposed && epoch === canvasEpoch) recordCompactionUsage(conv, e.payload, sel.providerId, sel.modelId, legacyCanvasRuns(conv))
  })
  try {
    const result = await invoke<PreparedContext>('prepare_library_chat_context', {
      ...contextArguments(conv, sel, canvasHistory(conv, sel)), mode: 'manual', eventName: `canvas-context-${requestId}`, requestId,
    })
    if (disposed || epoch !== canvasEpoch || stoppedRequests.has(requestId) || !conversations.value.includes(conv)) return
    if (fingerprint !== historyFingerprint(rawContextHistory(conv.messages, conv.id))) { contextError.value = t('chatContext.changed'); return }
    if (result?.usage) setContextUsage(conv, sel, result.usage)
    if (result?.compacted) {
      setContextCheckpoint(conv, sel, bound, result.messages)
      conv.updatedAt = new Date().toISOString(); scheduleSave()
      contextNotice.value = t('chatContext.compacted')
    } else contextNotice.value = t('chatContext.noRange')
  } catch (error) {
    if (!disposed && epoch === canvasEpoch && !stoppedRequests.has(requestId)) contextError.value = String(error)
  } finally {
    offUsage()
    if (!disposed && epoch === canvasEpoch) scheduleSave()
    stoppedRequests.delete(requestId)
    if (activeRequestId === requestId) { activeRequestId = null; loading.value = false; contextCompacting.value = false }
  }
}

function detachListeners() {
  for (const off of unlisteners) off()
  unlisteners = []
}

async function send() {
  const text = input.value.trim()
  const conv = activeConv.value
  const sel = model.value
  if (!text || !conv || !sel || loading.value || !libraryRoot.value) return
  const epoch = canvasEpoch, root = libraryRoot.value, canvasId = props.canvasId

  input.value = ''
  errorText.value = ''
  const now = new Date().toISOString()
  conv.messages.push({ id: newId(), role: 'user', content: text, createdAt: now })
  if (!conv.title) conv.title = text.slice(0, 40)
  const answer: CanvasMessage = {
    id: newId(),
    role: 'assistant',
    content: '',
    createdAt: now,
    streaming: true,
  }
  conv.messages.push(answer)
  // Reactivity tracks the proxy the array hands back, not the plain object we
  // pushed — writing to `answer` directly would stream into an object nothing
  // is watching, and the panel would sit on "reading the canvas…" until the
  // final save happened to re-render it. Everything below writes to `live`.
  const live = conv.messages[conv.messages.length - 1]
  conv.updatedAt = now
  loading.value = true
  scrollToBottom()

  // History as the model sees it: the canvas context, then the turns so far
  // (the empty answer we just pushed is not one of them).
  const prefix = conv.messages.slice(0, -1), bound = prefix[prefix.length - 1].id
  const fingerprint = historyFingerprint(rawContextHistory(prefix, conv.id))
  const prefixUnchanged = () => {
    const index = conv.messages.findIndex(m => m.id === bound)
    return index >= 0 && fingerprint === historyFingerprint(rawContextHistory(conv.messages.slice(0, index + 1), conv.id))
  }
  let history = canvasHistory(conv, sel, prefix)

  const eventName = `canvas-chat-${props.canvasId.replace(/[^A-Za-z0-9:_/-]/g, '-')}-${live.id}`
  const requestId = newId()
  const run = beginChatRun(conv, { id: requestId, answerId: live.id, providerId: sel.providerId, modelId: sel.modelId, modelCalls: 0 }, legacyCanvasRuns(conv))
  const runStarted = performance.now()
  const cacheReported = !!ai.settings.providers.find(p => p.id === sel.providerId)?.base_url.toLowerCase().includes('deepseek')
  activeRequestId = requestId
  const current = () => !disposed && epoch === canvasEpoch && root === libraryRoot.value && activeRequestId === requestId && !stoppedRequests.has(requestId)
  let pendingCheckpoint: ChatMessage[] | undefined
  detachListeners()

  unlisteners.push(await listen<StreamUsagePayload>(`${eventName}-run-usage`, e => {
    if (!disposed && epoch === canvasEpoch) applyRunUsage(run, e.payload, cacheReported)
  }))
  unlisteners.push(await listen<StreamUsagePayload>(`${eventName}-usage`, e => {
    if (disposed || epoch !== canvasEpoch) return
    applyRunUsage(run, e.payload, cacheReported)
    if (!run.modelCalls) run.modelCalls = 1
    live.inputTokens = run.inputTokens; live.outputTokens = run.outputTokens
    live.totalTokens = run.totalTokens; live.cacheHitTokens = run.cacheHitTokens
  }))
  unlisteners.push(await listen<StreamUsagePayload & { id: string }>(`${eventName}-compaction-usage`, e => {
    if (!disposed && epoch === canvasEpoch) recordCompactionUsage(conv, e.payload, sel.providerId, sel.modelId, [], cacheReported)
  }))
  unlisteners.push(await listen<ContextUsage>(`${eventName}-context-usage`, e => { if (current()) setContextUsage(conv, sel, e.payload) }))
  unlisteners.push(await listen<{ messages: ChatMessage[] }>(`${eventName}-context-checkpoint`, e => { if (current()) pendingCheckpoint = e.payload.messages }))
  unlisteners.push(await listen<{ phase: string; error?: string }>(`${eventName}-context-status`, e => {
    if (!current()) return
    contextCompacting.value = e.payload.phase === 'compacting'
    if (e.payload.phase === 'error') contextError.value = e.payload.error ?? ''
  }))

  unlisteners.push(await listen<{ delta?: string; done?: boolean }>(eventName, (e) => {
    if (e.payload?.done) return
    const delta = e.payload?.delta ?? ''
    if (!delta) return
    live.content += delta
    scheduleStreamRender(live)
  }))

  // OpenRouter's server tools report what they consulted or drew. The agent loop
  // emits one of these per round, so they are merged rather than replaced.
  unlisteners.push(await listen<ServerToolTrace>(`${eventName}-servertools`, (e) => {
    live.serverTools = mergeServerToolTrace(live.serverTools, e.payload)
  }))

  unlisteners.push(await listen<{
    phase?: string; tool?: string; arguments?: unknown; ok?: boolean; round?: number
    preview?: string; chars?: number; truncated?: boolean
    images?: AgentStepImage[] | null
  }>(
    `${eventName}-agent`,
    (e) => {
      const p = e.payload
      if (!p) return
      if (typeof p.round === 'number') run.modelCalls = Math.max(run.modelCalls ?? 0, p.round)
      if (p.phase === 'tool') run.toolCalls = (run.toolCalls ?? 0) + 1
      if (p.phase === 'tool') {
        if (!live.steps) live.steps = []
        live.steps.push({ tool: p.tool ?? '', args: summarizeArgs(p.arguments), argsJson: JSON.stringify(p.arguments ?? {}) })
        scrollToBottom()
      } else if (p.phase === 'result') {
        const step = [...(live.steps ?? [])].reverse().find(s => s.tool === p.tool && s.ok === undefined)
        if (step) {
          step.ok = p.ok ?? true
          step.preview = p.preview
          step.chars = p.chars
          step.truncated = p.truncated
          if (p.images?.length) {
            step.images = p.images
            scheduleSave()
            if (activeConvId.value === conv.id) scrollToBottom()
          }
        }
      }
    },
  ))

  // The agent asking to write. A note card renders in place; a canvas edit is
  // drawn on the canvas (via the store) and this card is just the legend and the
  // yes/no. `-confirm-close` clears it when the backend gave up waiting.
  unlisteners.push(await listen<{ requestId: string; preview: AgentWritePreview | CanvasEditPreview }>(
    `${eventName}-confirm`,
    (e) => {
      const p = e.payload
      if (!p?.requestId || !p.preview) return
      if (p.preview.tool === 'edit_canvas') {
        const preview = p.preview as CanvasEditPreview
        pendingWrites.value = [...pendingWrites.value, { kind: 'canvas', requestId: p.requestId, preview }]
        // Ask the matching canvas panel to draw the change as a preview.
        canvasStore.proposeCanvasEdit(p.requestId, preview.canvasId, preview.ops)
      } else {
        pendingWrites.value = [...pendingWrites.value, { kind: 'note', requestId: p.requestId, preview: p.preview as AgentWritePreview }]
      }
    },
  ))
  unlisteners.push(await listen<{ requestId: string }>(`${eventName}-confirm-close`, (e) => {
    const id = e.payload?.requestId
    if (!id) return
    const closing = pendingWrites.value.find(w => w.requestId === id)
    if (closing?.kind === 'canvas') canvasStore.clearCanvasEdit(id)
    pendingWrites.value = pendingWrites.value.filter(w => w.requestId !== id)
  }))

  try {
    if (!current()) return
    const prepared = await invoke<PreparedContext>('prepare_library_chat_context', {
      ...contextArguments(conv, sel, history), mode: 'auto', eventName, requestId,
    })
    if (!current()) return
    if (prepared?.usage) setContextUsage(conv, sel, prepared.usage)
    if (prepared?.compacted) {
      history = [canvasContext(), ...prepared.messages]
      if (prefixUnchanged()) setContextCheckpoint(conv, sel, bound, prepared.messages)
    }
    const final = await invoke<string>('chat_with_library', {
      messages: history,
      providerId: sel.providerId,
      modelId: sel.modelId,
      eventName,
      sourcesEventName: `${eventName}-sources`,
      knowledgeSource: 'agent',
      selectedPaperSlugs: [],
      attachments: null,
      useReasoning: false,
      reasoningEffort: null,
      requestId,
      webSearch: false,
      agentMaxRounds: null,
      conversationId: conv.id,
      // Anchors this answer to the canvas: it is what unlocks the `edit_canvas`
      // tool and the only canvas any edit can touch.
      canvasId,
      manageContext: true,
      // The calibrated count prepare admitted, for the agent's first round.
      previousUsage: prepared?.usage ?? null,
      expectedRoot: root,
    })
    if (current()) {
      if (final && !live.content) live.content = final
      live.streaming = false
      if (pendingCheckpoint && prefixUnchanged()) setContextCheckpoint(conv, sel, live.id, pendingCheckpoint)
    }
  } catch (e) {
    const message = String(e)
    // Stopping is the user's own doing, not a failure to report.
    if (current() && !/cancel/i.test(message)) {
      live.error = true
      live.content = live.content || message
      errorText.value = message
    }
  } finally {
    flushStreamRender(live)
    run.durationMs = performance.now() - runStarted
    live.streaming = false
    if (activeRequestId === requestId) { loading.value = false; contextCompacting.value = false; activeRequestId = null; detachListeners() }
    stoppedRequests.delete(requestId)
    conv.updatedAt = new Date().toISOString()
    // An answer that produced nothing at all (stopped immediately) would leave
    // an empty bubble behind.
    const hasImages = live.steps?.some(step =>
      step.tool === SEND_PAPER_IMAGE_TOOL && step.ok === true && step.images?.length,
    )
    if (!live.content.trim() && !live.error && !hasImages) {
      conv.messages = conv.messages.filter(m => m.id !== live.id)
    }
    if (!disposed && epoch === canvasEpoch && root === libraryRoot.value) scheduleSave()
    scrollToBottom()
  }
}

function stop() {
  if (!activeRequestId) return
  stoppedRequests.add(activeRequestId)
  invoke('cancel_ai_request', { requestId: activeRequestId }).catch(() => {})
}

function summarizeArgs(args: unknown): string {
  if (!args || typeof args !== 'object') return ''
  const entries = Object.entries(args as Record<string, unknown>)
    .filter(([, v]) => v !== null && v !== undefined && v !== '')
    .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
  const text = entries.join(', ')
  return text.length > 80 ? `${text.slice(0, 80)}…` : text
}

// ── Write confirmations ──────────────────────────────────────────────────────

function answerWrite(approved: boolean) {
  const pending = currentWrite.value
  if (!pending) return
  pendingWrites.value = pendingWrites.value.slice(1)
  // A canvas edit is applied (or dropped) by the panel that drew it.
  if (pending.kind === 'canvas') {
    canvasStore.resolveCanvasEdit(pending.requestId, approved ? 'apply' : 'discard')
  }
  invoke('resolve_agent_write', { requestId: pending.requestId, approved }).catch(() => {})
}

// ── Conversations ────────────────────────────────────────────────────────────

function startNewConversation() {
  historyOpen.value = false
  // An untouched blank conversation is already "new".
  const blank = conversations.value.find(c => c.messages.length === 0)
  if (blank) { activeConvId.value = blank.id; return }
  const conv = blankConversation()
  conversations.value.unshift(conv)
  activeConvId.value = conv.id
}

function openConversation(id: string) {
  activeConvId.value = id
  historyOpen.value = false
  scrollToBottom()
}

function deleteConversation(id: string) {
  conversations.value = conversations.value.filter(c => c.id !== id)
  if (activeConvId.value === id) {
    const next = conversations.value[0] ?? blankConversation()
    if (!conversations.value.includes(next)) conversations.value.unshift(next)
    activeConvId.value = next.id
  }
  scheduleSave()
}

// Coalesced to one scroll per frame: reading `scrollHeight` forces a synchronous
// layout of the whole message list, so calling it per streamed token cost more
// than the render it followed. rAF runs after Vue has patched the DOM, so the
// height it reads is the new one.
let scrollFramePending = false
function scrollToBottom() {
  if (scrollFramePending) return
  scrollFramePending = true
  requestAnimationFrame(() => {
    scrollFramePending = false
    const el = messagesEl.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

// ── Throttled streaming render ────────────────────────────────────────────────
// The answer is repainted from a `displayContent` copy refreshed on an interval,
// not on every token. Mirrors LibraryChat / AiTab.
const STREAM_RENDER_MS = 90
let streamRenderTimer: ReturnType<typeof setTimeout> | null = null
let streamRenderLast = 0

function applyStreamRender(msg: CanvasMessage) {
  msg.displayContent = msg.content
  scrollToBottom()
}

function scheduleStreamRender(msg: CanvasMessage) {
  const elapsed = Date.now() - streamRenderLast
  if (elapsed >= STREAM_RENDER_MS) {
    streamRenderLast = Date.now()
    applyStreamRender(msg)
    return
  }
  if (streamRenderTimer) return
  streamRenderTimer = setTimeout(() => {
    streamRenderTimer = null
    streamRenderLast = Date.now()
    applyStreamRender(msg)
  }, STREAM_RENDER_MS - elapsed)
}

/** The answer text to paint: the throttled copy while it streams, the real one
 *  once it is done. */
function streamText(msg: CanvasMessage) {
  return (msg.streaming ? msg.displayContent ?? msg.content : msg.content) ?? ''
}

/** Final flush, so the last tokens show even if a throttle window was pending. */
function flushStreamRender(msg: CanvasMessage) {
  if (streamRenderTimer) { clearTimeout(streamRenderTimer); streamRenderTimer = null }
  streamRenderLast = 0
  msg.displayContent = msg.content
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    void send()
  }
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

watch(() => props.canvasId, (id) => {
  // Leaving a canvas mid-answer: stop it rather than streaming into a
  // conversation the user can no longer see.
  if (loading.value) stop()
  canvasEpoch++
  contextCompacting.value = false
  detachListeners()
  // Any edit still parked on the canvas we are leaving is dropped, preview and all.
  for (const pending of pendingWrites.value) {
    if (pending.kind === 'canvas') canvasStore.clearCanvasEdit(pending.requestId)
    invoke('resolve_agent_write', { requestId: pending.requestId, approved: false }).catch(() => {})
  }
  pendingWrites.value = []
  loading.value = false
  void load(id)
}, { immediate: true })

watch(() => ai.defaultSelection, (sel) => {
  if (!model.value && sel) model.value = sel
}, { immediate: true })

void ai.load().then(() => {
  if (!model.value) model.value = ai.defaultSelection
})

onMounted(async () => {
  document.addEventListener('mousedown', closeModelMenu)
  const off = await listen<string>('library-changed', event => {
    if (event.payload === libraryRoot.value) return
    stop(); canvasEpoch++; libraryRevision++; loading.value = false; contextCompacting.value = false
    libraryRoot.value = event.payload
    knownConvIds.clear()
    void load(props.canvasId)
  })
  if (disposed) off()
  else offLibrary = off
  const revision = libraryRevision
  const root = await invoke<string | null>('get_current_library').catch(() => null)
  if (!disposed && revision === libraryRevision) libraryRoot.value = root
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeModelMenu)
  if (loading.value) stop()
  disposed = true; canvasEpoch++; contextInspection++
  offLibrary?.()
  if (contextInspectTimer) clearTimeout(contextInspectTimer)
  detachListeners()
  // Release any write still parked on this tab; the backend would otherwise
  // wait out its timeout before telling the model nothing was written.
  for (const pending of pendingWrites.value) {
    if (pending.kind === 'canvas') canvasStore.clearCanvasEdit(pending.requestId)
    invoke('resolve_agent_write', { requestId: pending.requestId, approved: false }).catch(() => {})
  }
  pendingWrites.value = []
  if (saveTimer) { clearTimeout(saveTimer); void save() }
})
</script>

<template>
  <div class="cc-root">
    <!-- Header: which canvas, plus conversation controls -->
    <div class="cc-header">
      <div class="cc-title">
        <Icon icon="fluent:chat-sparkle-24-regular" width="15" height="15" />
        <span class="cc-canvas-name" :title="canvasName">{{ canvasName || t('canvasChat.untitled') }}</span>
      </div>
      <div class="cc-header-actions">
        <button class="cc-icon-btn" :title="t('canvasChat.newChat')" @click="startNewConversation">
          <Icon icon="fluent:add-24-regular" width="15" height="15" />
        </button>
        <button
          class="cc-icon-btn"
          :class="{ active: historyOpen }"
          :title="t('canvasChat.history')"
          @click="historyOpen = !historyOpen"
        >
          <Icon icon="fluent:history-24-regular" width="15" height="15" />
        </button>
      </div>
    </div>

    <!-- History -->
    <div v-if="historyOpen" class="cc-history">
      <p v-if="conversations.every(c => c.messages.length === 0)" class="cc-history-empty">
        {{ t('canvasChat.noHistory') }}
      </p>
      <div
        v-for="conv in conversations.filter(c => c.messages.length > 0)"
        :key="conv.id"
        class="cc-history-item"
        :class="{ active: conv.id === activeConvId }"
        @click="openConversation(conv.id)"
      >
        <span class="cc-history-title">{{ conv.title || t('canvasChat.untitledChat') }}</span>
        <button class="cc-icon-btn tiny" :title="t('canvasChat.delete')" @click.stop="deleteConversation(conv.id)">
          <Icon icon="fluent:delete-24-regular" width="13" height="13" />
        </button>
      </div>
    </div>

    <!-- Model picker: floats over the thread, like the paper AI tab's -->
    <div ref="modelPickerRoot" class="cc-model-picker">
      <button class="cc-model-trigger" @click.stop="toggleModelMenu()">
        <span class="cc-model-icon">
          <img v-if="logoOf(selectedOption)" :src="logoOf(selectedOption)" alt="" />
          <span v-else class="cc-model-fallback">{{ selectedLabel.charAt(0).toUpperCase() }}</span>
        </span>
        <span class="cc-model-label">{{ selectedLabel }}</span>
        <Icon class="cc-chevron" :class="{ open: modelMenuOpen }" icon="fluent:chevron-down-24-regular" width="12" height="12" />
      </button>

      <div v-if="modelMenuOpen" class="cc-model-menu">
        <div v-for="group in ai.groupedModels" :key="group.id" class="cc-model-group">
          <div class="cc-model-group-name">
            <span>{{ group.name }}</span>
            <ProviderBalanceTag :provider-id="group.id" />
          </div>
          <button
            v-for="option in group.models"
            :key="`${option.providerId}::${option.modelId}`"
            class="cc-model-row"
            :class="{ active: isSelected(option) }"
            @click="pickModel(option)"
          >
            <span class="cc-model-row-icon">
              <img v-if="logoOf(option)" :src="logoOf(option)" alt="" />
              <span v-else>{{ option.displayName.charAt(0).toUpperCase() }}</span>
            </span>
            <span class="cc-model-row-text">
              <span class="cc-model-row-name">
                {{ option.displayName }}
                <span
                  v-if="offerOf(option)"
                  class="cc-offer-tag"
                  :class="[offerOf(option)!.kind, { idle: !offerOf(option)!.activeNow }]"
                  :title="offerOf(option)!.title"
                >{{ offerOf(option)!.label }}</span>
              </span>
              <span class="cc-model-row-meta">
                <span class="cc-row-size" :class="{ assumed: !sizeOf(option).known }" :title="sizeOf(option).title">{{ sizeOf(option).text }}</span>
                {{ modelCapabilityText(option) || option.modelId }}
              </span>
            </span>
          </button>
        </div>
      </div>
    </div>

    <!-- Messages -->
    <div ref="messagesEl" class="cc-messages">
      <div v-if="messages.length === 0" class="cc-empty">
        <!-- Someone talking through a diagram — the canvas equivalent of the
             reader in the paper panel. -->
        <Icon icon="doodle:explaining-whiteboard" width="66" height="66" />
        <p class="cc-empty-title">{{ t('canvasChat.emptyTitle') }}</p>
        <ul class="cc-empty-hints">
          <li>{{ t('canvasChat.hint1') }}</li>
          <li>{{ t('canvasChat.hint2') }}</li>
          <li>{{ t('canvasChat.hint3') }}</li>
        </ul>
      </div>

      <div v-for="msg in messages" :key="msg.id" class="cc-msg" :class="msg.role">
        <div v-if="msg.role === 'user'" class="cc-user-bubble">{{ msg.content }}</div>
        <template v-else>
          <!-- What the model looked at, so an answer about the canvas is checkable -->
          <div v-if="msg.steps?.length" class="cc-steps">
            <div v-for="(step, i) in msg.steps" :key="i" class="cc-step">
              <Icon
                :icon="step.ok === false ? 'fluent:error-circle-24-regular' : step.ok === undefined ? 'fluent:arrow-clockwise-24-regular' : 'fluent:checkmark-24-regular'"
                width="12"
                height="12"
                :class="{ spin: step.ok === undefined }"
              />
              <span class="cc-step-tool">{{ step.tool }}</span>
              <span v-if="step.args" class="cc-step-args">{{ step.args }}</span>
            </div>
          </div>
          <div class="cc-answer" :class="{ error: msg.error }">
            <ChatAnswerBody
              :content="streamText(msg)"
              :streaming="msg.streaming"
              :conversation-id="activeConvId"
              :steps="msg.steps"
              @open="previewImage = $event"
            />
            <span v-if="msg.streaming && !streamText(msg)" class="cc-thinking">{{ t('canvasChat.thinking') }}</span>
            <ServerToolTraceCard :trace="msg.serverTools" />
          </div>
        </template>
      </div>
    </div>

    <!-- Composer -->
    <div class="cc-composer-wrap">
      <CanvasEditConfirmCard
        v-if="currentCanvasWrite"
        :key="currentCanvasWrite.requestId"
        :preview="currentCanvasWrite.preview"
        :queued="pendingWrites.length - 1"
        @approve="answerWrite(true)"
        @reject="answerWrite(false)"
      />
      <WriteConfirmCard
        v-else-if="currentNoteWrite"
        :key="currentNoteWrite.requestId"
        :preview="currentNoteWrite.preview"
        :queued="pendingWrites.length - 1"
        @approve="answerWrite(true)"
        @reject="answerWrite(false)"
      />

      <div v-if="!ai.isConfigured" class="cc-no-model">{{ t('canvasChat.noModel') }}</div>

      <div class="cc-composer">
        <textarea
          v-model="input"
          class="cc-input"
          rows="2"
          :placeholder="t('canvasChat.placeholder')"
          @keydown="onKeydown"
        />
        <div class="cc-composer-footer">
          <span class="cc-spacer" />
          <ContextMeter :key="activeConvId ?? 'new'" :usage="contextDisplay" :busy="loading" :compacting="contextCompacting"
            :error="contextError" :notice="contextNotice" @inspect="inspectContext" @compact="compactContext" />
          <button v-if="loading" class="cc-send stop" @click="stop">
            <Icon icon="fluent:stop-24-filled" width="13" height="13" />
            {{ t('canvasChat.stop') }}
          </button>
          <button v-else class="cc-send" :disabled="!canSend" @click="send">
            <Icon icon="fluent:send-24-filled" width="13" height="13" />
            {{ t('canvasChat.send') }}
          </button>
        </div>
      </div>
    </div>
  </div>
  <Teleport to="body">
    <div v-if="previewImage" class="cc-image-lightbox" @click.self="previewImage = null">
      <img :src="previewImage" :alt="t('chat.imageEnlarge')" />
      <button type="button" :title="t('chat.imageClose')" @click="previewImage = null">
        <Icon icon="fluent:dismiss-24-regular" width="20" height="20" />
      </button>
    </div>
  </Teleport>
</template>

<style scoped>
.cc-image-lightbox {
  position: fixed;
  inset: 0;
  z-index: 1200;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.65);
}
.cc-image-lightbox img {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  border-radius: var(--radius-md);
}
.cc-image-lightbox button {
  position: absolute;
  top: 12px;
  right: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 50%;
  background: var(--bg-secondary);
  color: var(--text-primary);
}
.cc-root {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--bg-primary);
}

/* ── Floating model picker ── */
.cc-model-picker {
  position: absolute;
  /* Same offset and size as the AI tab's picker: the two sit in the same slot
     of the same panel, so any difference reads as the pill jumping when the
     user switches tabs. */
  top: calc(var(--content-header-height) + 18px);
  left: 50%;
  transform: translateX(-50%);
  /* Sized here rather than on the button: the button's own `100%` would
     resolve against this box, which is sized by its content. */
  width: min(240px, calc(100% - 24px));
  z-index: 20;
}
.cc-model-trigger {
  height: 36px;
  width: 100%;
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
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  cursor: pointer;
}
.cc-model-trigger:hover { background: var(--bg-hover); }
.cc-model-icon { display: flex; align-items: center; justify-content: center; }
.cc-model-icon img { width: 18px; height: 18px; border-radius: 5px; object-fit: contain; }
.cc-model-fallback {
  width: 18px;
  height: 18px;
  border-radius: 5px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  color: var(--accent);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
}
.cc-model-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12.5px;
  font-weight: 500;
  text-align: center;
}
.cc-chevron { color: var(--text-tertiary); transition: transform 0.16s ease; }
.cc-chevron.open { transform: rotate(180deg); }

.cc-model-menu {
  position: absolute;
  top: calc(100% + 6px);
  left: 50%;
  transform: translateX(-50%);
  width: min(290px, calc(100vw - 40px));
  max-height: min(420px, 60vh);
  overflow-y: auto;
  padding: 7px;
  border: 1px solid var(--border-subtle);
  border-radius: 13px;
  background: color-mix(in srgb, var(--bg-primary) 96%, var(--bg-secondary));
  box-shadow: 0 18px 44px rgba(15, 23, 42, 0.18);
}
.cc-model-group + .cc-model-group {
  margin-top: 7px;
  padding-top: 7px;
  border-top: 1px solid var(--border-subtle);
}
.cc-model-group-name {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 3px 8px 5px;
  font-size: 11px;
  font-weight: 700;
  color: var(--text-tertiary);
}
.cc-model-row {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 7px 8px;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: var(--text-secondary);
  text-align: left;
  cursor: pointer;
}
.cc-model-row:hover { background: var(--bg-hover); color: var(--text-primary); }
.cc-model-row.active { background: color-mix(in srgb, var(--accent) 12%, transparent); color: var(--accent); }
.cc-model-row-icon { display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
.cc-model-row-icon img { width: 18px; height: 18px; border-radius: 5px; object-fit: contain; }
.cc-model-row-icon span {
  width: 18px;
  height: 18px;
  border-radius: 5px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 10px;
  font-weight: 700;
  background: var(--bg-secondary);
}
.cc-model-row-text { min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.cc-model-row-name {
  font-size: 12.5px;
  font-weight: 650;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cc-model-row.active .cc-model-row-name { color: var(--accent); }
.cc-model-row-meta {
  font-size: 10.5px;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* Plain text, not a filled chip — see the note in AiTab. */
.cc-row-size { margin-right: 5px; font-weight: 600; }
.cc-row-size::after { content: '·'; margin-left: 5px; font-weight: 400; opacity: 0.55; }
.cc-row-size.assumed { opacity: 0.6; font-weight: 500; }
.cc-offer-tag {
  display: inline-block;
  margin-left: 5px;
  padding: 0 5px;
  border-radius: var(--radius-sm);
  font-size: 9.5px;
  font-weight: 600;
  line-height: 15px;
  vertical-align: middle;
}
.cc-offer-tag.free { color: #15803d; background: color-mix(in srgb, #22c55e 16%, transparent); }
.cc-offer-tag.discount { color: #b45309; background: color-mix(in srgb, #f59e0b 18%, transparent); }
.cc-offer-tag.discount.idle { color: var(--text-tertiary); background: color-mix(in srgb, var(--text-tertiary) 12%, transparent); }

.cc-header {
  height: var(--content-header-height);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 0 10px 0 12px;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-secondary) 68%, var(--bg-primary));
  flex-shrink: 0;
}
.cc-title { display: flex; align-items: center; gap: 7px; min-width: 0; }
.cc-title svg { color: var(--accent); flex-shrink: 0; }
.cc-canvas-name {
  font-size: var(--font-size-sm);
  font-weight: 650;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cc-header-actions { margin-left: auto; display: flex; gap: 6px; flex-shrink: 0; }
.cc-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 5px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  background: transparent;
  border: none;
  cursor: pointer;
}
.cc-icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.cc-icon-btn.active { background: var(--bg-active); color: var(--accent); }
.cc-icon-btn.tiny { padding: 3px; }

/* Header buttons match the AI tab's .icon-btn: a fixed 30px outlined box that
   turns accent on hover — while the base .cc-icon-btn stays compact for the
   history list's inline delete (.tiny), which is not inside .cc-header-actions. */
.cc-header-actions .cc-icon-btn {
  width: 30px;
  height: 30px;
  padding: 0;
  border-radius: 9px;
  border: 1px solid transparent;
}
.cc-header-actions .cc-icon-btn:hover {
  color: var(--accent);
  border-color: var(--border-default);
  background: var(--bg-hover);
}

.cc-history {
  flex-shrink: 0;
  max-height: 180px;
  overflow-y: auto;
  padding: 6px;
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
}
.cc-history-empty { margin: 6px; font-size: 12px; color: var(--text-tertiary); }
.cc-history-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.cc-history-item:hover { background: var(--bg-hover); }
.cc-history-item.active { background: var(--bg-active); }
.cc-history-title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cc-messages {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  /* Top padding clears the floating model pill, which sits over this area. */
  padding: 56px 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.cc-empty {
  margin: auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding: 20px;
  text-align: center;
  color: var(--text-tertiary);
}
.cc-empty svg { color: color-mix(in srgb, var(--accent) 60%, var(--text-tertiary)); }
.cc-empty-title { margin: 0; font-size: 13px; color: var(--text-secondary); }
.cc-empty-hints {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.cc-empty-hints li {
  font-family: var(--font-hand);
  font-size: 12.5px;
  line-height: 1.6;
  letter-spacing: 0.2px;
  color: var(--text-tertiary);
}

.cc-msg { display: flex; flex-direction: column; gap: 6px; }
.cc-msg.user { align-items: flex-end; }
.cc-user-bubble {
  max-width: 88%;
  padding: 8px 11px;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  white-space: pre-wrap;
  word-break: break-word;
}

.cc-steps {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 7px 9px;
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
}
.cc-step { display: flex; align-items: center; gap: 6px; min-width: 0; font-size: 11px; }
.cc-step svg { flex-shrink: 0; color: var(--text-tertiary); }
.cc-step-tool { color: var(--text-secondary); font-family: var(--font-mono); }
.cc-step-args {
  min-width: 0;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.spin { animation: cc-spin 1.1s linear infinite; }
@keyframes cc-spin { to { transform: rotate(360deg); } }

.cc-answer {
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--text-primary);
  word-break: break-word;
}
.cc-answer.error { color: #dc2626; }
.cc-answer :deep(p) { margin: 6px 0; }
.cc-answer :deep(pre) { font-size: 11.5px; overflow-x: auto; }
.cc-answer :deep(img) { max-width: 100%; }
.cc-thinking { font-size: 12px; color: var(--text-tertiary); }

.cc-composer-wrap {
  flex-shrink: 0;
  padding: 10px 12px 12px;
  border-top: 1px solid var(--border-subtle);
}
.cc-no-model {
  margin-bottom: 8px;
  padding: 8px 10px;
  font-size: 11.5px;
  line-height: 1.5;
  color: #b45309;
  background: color-mix(in srgb, #f59e0b 10%, var(--bg-primary));
  border-radius: var(--radius-md);
}
.cc-composer {
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  padding: 8px 10px;
}
.cc-composer:focus-within { border-color: var(--accent); }
.cc-input {
  width: 100%;
  border: none;
  background: transparent;
  resize: none;
  font-family: inherit;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--text-primary);
}
.cc-input:focus { outline: none; }
.cc-composer-footer { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
.cc-spacer { flex: 1; }
.cc-send {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 550;
  color: #fff;
  background: var(--accent);
  border: none;
  border-radius: var(--radius-md);
  cursor: pointer;
}
.cc-send:disabled { opacity: 0.45; cursor: default; }
.cc-send.stop { background: var(--bg-tertiary); color: var(--text-primary); }
</style>
