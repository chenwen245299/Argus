import { reactive, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useAiStore } from './ai'
import { useSettingsStore } from './settings'

export interface TranslationEntry {
  id: string
  sourceText: string
  result: string
  createdAt: string
  // Model + token/cost usage (optional — older stored entries won't have these).
  providerName?: string
  modelName?: string
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  cacheHitTokens?: number
  costUsd?: number | null
}

// Raw usage payload emitted by the backend (`translate-stream-{id}-usage`).
interface StreamUsagePayload {
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cost_usd?: number | null
  cache_hit_tokens?: number
}

const MAX_HISTORY = 200
const STORAGE_KEY = 'argus:translation-history'

function loadFromStorage(): TranslationEntry[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    return JSON.parse(raw) as TranslationEntry[]
  } catch {
    return []
  }
}

function saveToStorage(entries: TranslationEntry[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(entries))
  } catch {}
}

export const translationHistory = reactive<{ entries: TranslationEntry[] }>({
  entries: loadFromStorage(),
})

// Current in-progress (or most recent) translation — drives the tab's main view
export const currentTranslation = reactive({
  sourceText: '',
  result: '',
  loading: false,
  error: '',
  // Model used for this translation
  providerId: '',
  modelId: '',
  providerName: '',
  modelName: '',
  // Token/cost usage (filled from the backend usage event)
  inputTokens: undefined as number | undefined,
  outputTokens: undefined as number | undefined,
  totalTokens: undefined as number | undefined,
  cacheHitTokens: undefined as number | undefined,
  costUsd: undefined as number | null | undefined,
  // Timings for a tok/s estimate
  startedAt: 0,
  endedAt: 0,
})

// Signal MainView to switch to translations tab
export const switchToTranslationsTab = ref(false)

// Signal MainView + AiTab to open AI chat with pre-filled text
export const askAiText = ref<string | null>(null)

export function triggerAskAi(text: string) {
  askAiText.value = text
}

interface TranslationModel {
  providerId: string
  modelId: string
  providerName: string
  modelName: string
}

export function startTranslation(sourceText: string, model?: TranslationModel) {
  currentTranslation.sourceText = sourceText
  currentTranslation.result = ''
  currentTranslation.loading = true
  currentTranslation.error = ''
  currentTranslation.providerId = model?.providerId ?? ''
  currentTranslation.modelId = model?.modelId ?? ''
  currentTranslation.providerName = model?.providerName ?? ''
  currentTranslation.modelName = model?.modelName ?? ''
  currentTranslation.inputTokens = undefined
  currentTranslation.outputTokens = undefined
  currentTranslation.totalTokens = undefined
  currentTranslation.cacheHitTokens = undefined
  currentTranslation.costUsd = undefined
  currentTranslation.startedAt = performance.now()
  currentTranslation.endedAt = 0
  switchToTranslationsTab.value = true
}

export function appendTranslationChunk(delta: string) {
  currentTranslation.result += delta
  // loading stays true during streaming; only finish/fail clears it.
}

export function applyTranslationUsage(usage: StreamUsagePayload) {
  if (typeof usage.input_tokens === 'number') currentTranslation.inputTokens = usage.input_tokens
  if (typeof usage.output_tokens === 'number') currentTranslation.outputTokens = usage.output_tokens
  if (typeof usage.total_tokens === 'number') currentTranslation.totalTokens = usage.total_tokens
  if (typeof usage.cache_hit_tokens === 'number') currentTranslation.cacheHitTokens = usage.cache_hit_tokens
  if (typeof usage.cost_usd === 'number' || usage.cost_usd === null) currentTranslation.costUsd = usage.cost_usd
}

export function finishTranslation() {
  currentTranslation.loading = false
  currentTranslation.endedAt = performance.now()
  if (!currentTranslation.result) return
  const entry: TranslationEntry = {
    id: crypto.randomUUID(),
    sourceText: currentTranslation.sourceText,
    result: currentTranslation.result,
    createdAt: new Date().toISOString(),
    providerName: currentTranslation.providerName,
    modelName: currentTranslation.modelName,
    inputTokens: currentTranslation.inputTokens,
    outputTokens: currentTranslation.outputTokens,
    totalTokens: currentTranslation.totalTokens,
    cacheHitTokens: currentTranslation.cacheHitTokens,
    costUsd: currentTranslation.costUsd,
  }
  translationHistory.entries.unshift(entry)
  if (translationHistory.entries.length > MAX_HISTORY) {
    translationHistory.entries.splice(MAX_HISTORY)
  }
  saveToStorage(translationHistory.entries)
}

export function failTranslation(error: string) {
  currentTranslation.loading = false
  currentTranslation.endedAt = performance.now()
  currentTranslation.error = error
}

// ── Streaming translation runner ────────────────────────────────────────────
// Centralized here (rather than in PdfViewer) so any view — including the
// translation tab's "regenerate" button — can start a translation.

let _translateUnlisten: (() => void) | null = null

function cleanupTranslateListeners() {
  _translateUnlisten?.()
  _translateUnlisten = null
}

export async function runTranslation(sourceText: string) {
  if (!sourceText.trim() || currentTranslation.loading) return

  // Resolve the configured translation model, mirroring the Rust fallback:
  // explicit translate model → global default model. Best-effort for display;
  // the backend uses the same settings so it stays in sync.
  const ai = useAiStore()
  const settings = useSettingsStore()
  const providerId = settings.settings.translate_ai_provider_id || ai.settings.default_provider_id || ''
  const modelId = settings.settings.translate_ai_model_id || ai.settings.default_model_id || ''
  const model = ai.enabledModels.find(m => m.providerId === providerId && m.modelId === modelId)

  startTranslation(sourceText, {
    providerId,
    modelId,
    providerName: model?.providerName ?? '',
    modelName: model?.displayName ?? modelId,
  })

  cleanupTranslateListeners()

  const eventId = crypto.randomUUID()
  try {
    const offUsage = await listen<StreamUsagePayload>(
      `translate-stream-${eventId}-usage`,
      ({ payload }) => applyTranslationUsage(payload),
    )
    const offStream = await listen<{ delta: string; done: boolean }>(
      `translate-stream-${eventId}`,
      ({ payload }) => {
        if (payload.done) {
          finishTranslation()
          cleanupTranslateListeners()
          return
        }
        appendTranslationChunk(payload.delta)
      },
    )
    _translateUnlisten = () => { offUsage(); offStream() }
    await invoke('translate_text_stream', { text: sourceText, eventId })
  } catch (e) {
    failTranslation(String(e))
    cleanupTranslateListeners()
  }
}

export function regenerateTranslation() {
  if (currentTranslation.sourceText) runTranslation(currentTranslation.sourceText)
}

export function deleteTranslationEntry(id: string) {
  const idx = translationHistory.entries.findIndex(e => e.id === id)
  if (idx !== -1) translationHistory.entries.splice(idx, 1)
  saveToStorage(translationHistory.entries)
}

export function clearTranslationHistory() {
  translationHistory.entries = []
  saveToStorage([])
}
