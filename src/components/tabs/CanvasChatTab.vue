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
import MarkdownBody from '../MarkdownBody.vue'
import WriteConfirmCard from '../WriteConfirmCard.vue'
import { modelLogo as logoFor, modelCapabilityText } from '../../utils/modelLogo'
import { modelOffer, modelSizeLabel } from '../../utils/modelOffers'
import type { AgentWritePreview, ChatMessage, ModelSelection } from '../../types'

const props = defineProps<{ canvasId: string }>()

const { t } = useI18n()
const ai = useAiStore()
const canvasStore = useCanvasStore()

interface AgentStep {
  tool: string
  args: string
  ok?: boolean
}

interface CanvasMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  createdAt: string
  /** Transient: true while this answer is still streaming. Never persisted. */
  streaming?: boolean
  error?: boolean
  steps?: AgentStep[]
}

interface CanvasConversation {
  id: string
  title: string
  messages: CanvasMessage[]
  createdAt: string
  updatedAt: string
}

const conversations = ref<CanvasConversation[]>([])
const activeConvId = ref<string | null>(null)
const input = ref('')
const loading = ref(false)
const historyOpen = ref(false)
const model = ref<ModelSelection | null>(null)
const messagesEl = ref<HTMLElement | null>(null)
const errorText = ref('')

// A write the agent asked for, waiting on the user. Same card the library chat
// uses; nothing is written until it is answered.
interface PendingWrite { requestId: string; preview: AgentWritePreview }
const pendingWrites = ref<PendingWrite[]>([])
const currentWrite = computed<PendingWrite | null>(() => pendingWrites.value[0] ?? null)

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
      messages: c.messages.map(({ streaming: _streaming, ...m }) => m),
    }))
}

function scheduleSave() {
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => { void save() }, 400)
}

async function save() {
  saveTimer = null
  const canvasId = props.canvasId
  try {
    await invoke('save_canvas_ai_conversations', {
      canvasId,
      conversations: persistable(conversations.value),
    })
  } catch (e) {
    console.error('save_canvas_ai_conversations:', e)
  }
}

async function load(canvasId: string) {
  conversations.value = []
  activeConvId.value = null
  errorText.value = ''
  try {
    const raw = await invoke<unknown>('get_canvas_ai_conversations', { canvasId })
    if (Array.isArray(raw)) {
      conversations.value = raw
        .filter((c): c is CanvasConversation => !!c && typeof c === 'object' && Array.isArray((c as CanvasConversation).messages))
        .map(c => ({ ...c, messages: c.messages.map(m => ({ ...m, streaming: false })) }))
        .sort((a, b) => (b.updatedAt ?? '').localeCompare(a.updatedAt ?? ''))
    }
  } catch (e) {
    console.error('get_canvas_ai_conversations:', e)
  }
  // Canvas changed under us while the read was in flight.
  if (canvasId !== props.canvasId) return
  const conv = conversations.value[0] ?? blankConversation()
  if (!conversations.value.includes(conv)) conversations.value.unshift(conv)
  activeConvId.value = conv.id
}

// ── Sending ──────────────────────────────────────────────────────────────────

function detachListeners() {
  for (const off of unlisteners) off()
  unlisteners = []
}

async function send() {
  const text = input.value.trim()
  const conv = activeConv.value
  const sel = model.value
  if (!text || !conv || !sel || loading.value) return

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
  const history: ChatMessage[] = [
    canvasContext(),
    ...conv.messages
      .filter(m => m.id !== live.id && !m.error)
      .map(m => ({ role: m.role, content: m.content })),
  ]

  const eventName = `canvas-chat-${props.canvasId.replace(/[^A-Za-z0-9:_/-]/g, '-')}-${live.id}`
  const requestId = newId()
  activeRequestId = requestId
  detachListeners()

  unlisteners.push(await listen<{ delta?: string; done?: boolean }>(eventName, (e) => {
    if (e.payload?.done) return
    const delta = e.payload?.delta ?? ''
    if (!delta) return
    live.content += delta
    scrollToBottom()
  }))

  unlisteners.push(await listen<{ phase?: string; tool?: string; arguments?: unknown; ok?: boolean }>(
    `${eventName}-agent`,
    (e) => {
      const p = e.payload
      if (!p) return
      if (p.phase === 'tool') {
        if (!live.steps) live.steps = []
        live.steps.push({ tool: p.tool ?? '', args: summarizeArgs(p.arguments) })
        scrollToBottom()
      } else if (p.phase === 'result') {
        const step = [...(live.steps ?? [])].reverse().find(s => s.tool === p.tool && s.ok === undefined)
        if (step) step.ok = p.ok ?? true
      }
    },
  ))

  // The agent asking to write a note. Handled exactly as in the library chat:
  // the card appears above the composer and nothing happens until it is
  // answered; `-confirm-close` clears it when the backend gave up waiting.
  unlisteners.push(await listen<{ requestId: string; preview: AgentWritePreview }>(
    `${eventName}-confirm`,
    (e) => {
      const p = e.payload
      if (!p?.requestId || !p.preview) return
      pendingWrites.value = [...pendingWrites.value, { requestId: p.requestId, preview: p.preview }]
    },
  ))
  unlisteners.push(await listen<{ requestId: string }>(`${eventName}-confirm-close`, (e) => {
    const id = e.payload?.requestId
    if (id) pendingWrites.value = pendingWrites.value.filter(w => w.requestId !== id)
  }))

  try {
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
    })
    if (final && !live.content) live.content = final
  } catch (e) {
    const message = String(e)
    // Stopping is the user's own doing, not a failure to report.
    if (!/cancel/i.test(message)) {
      live.error = true
      live.content = live.content || message
      errorText.value = message
    }
  } finally {
    live.streaming = false
    loading.value = false
    activeRequestId = null
    detachListeners()
    conv.updatedAt = new Date().toISOString()
    // An answer that produced nothing at all (stopped immediately) would leave
    // an empty bubble behind.
    if (!live.content.trim() && !live.error) {
      conv.messages = conv.messages.filter(m => m.id !== live.id)
    }
    scheduleSave()
    scrollToBottom()
  }
}

function stop() {
  if (!activeRequestId) return
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

function scrollToBottom() {
  void nextTick(() => {
    const el = messagesEl.value
    if (el) el.scrollTop = el.scrollHeight
  })
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
  detachListeners()
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

onMounted(() => {
  document.addEventListener('mousedown', closeModelMenu)
})

onUnmounted(() => {
  document.removeEventListener('mousedown', closeModelMenu)
  if (loading.value) stop()
  detachListeners()
  // Release any write still parked on this tab; the backend would otherwise
  // wait out its timeout before telling the model nothing was written.
  for (const pending of pendingWrites.value) {
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
        <Icon icon="doodle:chat-panel" width="16" height="16" />
        <span class="cc-canvas-name" :title="canvasName">{{ canvasName || t('canvasChat.untitled') }}</span>
      </div>
      <div class="cc-header-actions">
        <button class="cc-icon-btn" :title="t('canvasChat.newChat')" @click="startNewConversation">
          <Icon icon="fluent:compose-24-regular" width="15" height="15" />
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
      <button class="cc-model-trigger" @click.stop="modelMenuOpen = !modelMenuOpen">
        <span class="cc-model-icon">
          <img v-if="logoOf(selectedOption)" :src="logoOf(selectedOption)" alt="" />
          <span v-else class="cc-model-fallback">{{ selectedLabel.charAt(0).toUpperCase() }}</span>
        </span>
        <span class="cc-model-label">{{ selectedLabel }}</span>
        <Icon class="cc-chevron" :class="{ open: modelMenuOpen }" icon="fluent:chevron-down-24-regular" width="12" height="12" />
      </button>

      <div v-if="modelMenuOpen" class="cc-model-menu">
        <div v-for="group in ai.groupedModels" :key="group.id" class="cc-model-group">
          <div class="cc-model-group-name">{{ group.name }}</div>
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
            <MarkdownBody :content="msg.content" :streaming="msg.streaming" />
            <span v-if="msg.streaming && !msg.content" class="cc-thinking">{{ t('canvasChat.thinking') }}</span>
          </div>
        </template>
      </div>
    </div>

    <!-- Composer -->
    <div class="cc-composer-wrap">
      <WriteConfirmCard
        v-if="currentWrite"
        :key="currentWrite.requestId"
        :preview="currentWrite.preview"
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
</template>

<style scoped>
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
  top: 52px;
  left: 50%;
  transform: translateX(-50%);
  /* Sized here rather than on the button: the button's own `100%` would
     resolve against this box, which is sized by its content. */
  width: min(250px, calc(100% - 24px));
  z-index: 20;
}
.cc-model-trigger {
  height: 34px;
  width: 100%;
  display: grid;
  grid-template-columns: 18px 1fr 14px;
  align-items: center;
  gap: 7px;
  padding: 0 11px;
  border: 1px solid var(--border-default);
  border-radius: 11px;
  color: var(--text-primary);
  background: color-mix(in srgb, var(--bg-primary) 90%, transparent);
  backdrop-filter: blur(18px) saturate(1.35);
  -webkit-backdrop-filter: blur(18px) saturate(1.35);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.12);
  cursor: pointer;
}
.cc-model-trigger:hover { background: var(--bg-hover); }
.cc-model-icon { display: flex; align-items: center; justify-content: center; }
.cc-model-icon img { width: 17px; height: 17px; border-radius: 5px; object-fit: contain; }
.cc-model-fallback {
  width: 17px;
  height: 17px;
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
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.cc-title { display: flex; align-items: center; gap: 7px; min-width: 0; }
.cc-title svg { color: var(--accent); flex-shrink: 0; }
.cc-canvas-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cc-header-actions { margin-left: auto; display: flex; gap: 2px; flex-shrink: 0; }
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
