import { defineStore } from 'pinia'
import { ref, computed, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { i18n } from '../i18n'
import { useSettingsStore } from './settings'
import { useAiStore } from './ai'
import { prepareSpeechText, chunkForSpeech } from '../utils/speechText'
import {
  SpeechEngine, speechScope,
  type SpeechState, type SpeechInfo, type SpeechErrorReason, type SynthResult, type SynthSignal,
} from '../utils/speechEngine'
import type {
  AppSettings, MediaProviderCapabilities, MediaCapability, MediaModelSpec, MediaResult,
} from '../types'

/**
 * Read aloud (朗读).
 *
 * The store owns three things and nothing else:
 *
 * 1. WHICH voice — `speech_provider_id` / `speech_model_id` / `speech_options` in
 *    the app settings, checked against what the backend says it can still do
 *    (`list_media_capabilities`), so a provider that was deleted, disabled or lost
 *    its key reads as "not configured" rather than failing on the first click.
 * 2. HOW a selection becomes audio — `prepareSpeechText` -> `chunkForSpeech` ->
 *    the `SpeechEngine`, which synthesises through the provider-agnostic
 *    `run_media_task`. Nothing here names a provider: the model's own
 *    `maxPromptChars` and `fields` (as data) drive the chunking and the options.
 * 3. WHAT the UI shows — state, progress, the error text and the "not configured"
 *    prompt, which `SpeechHost.vue` renders.
 *
 * Entry point for a popup button: `speech.read(text)`, called straight from the
 * click handler (see `read`).
 */

export type SpeechSetupReason = 'unset' | 'provider-missing' | 'no-providers'

export interface SpeechConfig {
  providerId: string
  modelId: string
  options: Record<string, unknown>
  skipCitations: boolean
}

/** What is sent per request, frozen when a read starts (see `scopeConfigs`). */
interface RequestConfig {
  providerId: string
  model: string
  options: Record<string, unknown>
}

/** Used when a model does not declare `maxPromptChars`: short enough for any speech API. */
export const FALLBACK_MAX_CHARS = 500

/** Short, bilingual, a couple of seconds: enough to hear a voice without paying for a page. */
export const PREVIEW_TEXT = '你好，这是 Argus 的朗读试听。Hello, this is a read-aloud preview from Argus.'

/** The defaults a model declares for its knobs, as the Media Studio builds them. */
export function defaultOptions(model: MediaModelSpec | null | undefined): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const f of model?.fields ?? []) {
    if (f.default !== undefined && f.default !== null) out[f.key] = f.default
  }
  return out
}

/**
 * Stored values over the model's defaults, keeping only keys the model declares.
 * A cleared text box (`''`) means "the provider's own default", so it is left out
 * rather than sent as an empty string; a key from a model the user has since
 * switched away from is dropped, since the adapter never declared it.
 */
export function effectiveOptions(
  model: MediaModelSpec | null | undefined,
  stored: Record<string, unknown> | null | undefined,
): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const f of model?.fields ?? []) {
    const v = stored?.[f.key]
    if (v !== undefined && v !== null && v !== '') out[f.key] = v
    else if (f.default !== undefined && f.default !== null) out[f.key] = f.default
  }
  return out
}

function speechCapabilityOf(p: MediaProviderCapabilities): MediaCapability | undefined {
  return p.capabilities.find((c) => c.kind === 'speech' && c.models.length > 0)
}

const t = (key: string, params?: Record<string, unknown>): string =>
  i18n.global.t(key, params ?? {}) as string

/** `data:` URI from the bytes of a provider-hosted file (a future adapter may return a URL instead of bytes). */
async function downloadAsDataUrl(url: string, mime: string): Promise<string> {
  const bytes = await invoke<number[]>('fetch_media_artifact', { url })
  const blob = new Blob([new Uint8Array(bytes)], { type: mime })
  return await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result))
    reader.onerror = () => reject(reader.error ?? new Error('read failed'))
    reader.readAsDataURL(blob)
  })
}

export const useSpeechStore = defineStore('speech', () => {
  const settingsStore = useSettingsStore()
  const aiStore = useAiStore()

  // ── What the backend can do ────────────────────────────────────────────────

  /** Providers that can synthesise speech right now (enabled, with a key, with a speech model). */
  const providers = ref<MediaProviderCapabilities[]>([])
  const loaded = ref(false)
  const loading = ref(false)
  const loadError = ref('')
  let loadSeq = 0

  /** Resolves true when the list is fresh. Concurrent calls each get their own answer; the newest wins. */
  async function loadCapabilities(): Promise<boolean> {
    const seq = ++loadSeq
    loading.value = true
    try {
      const all = await invoke<MediaProviderCapabilities[]>('list_media_capabilities')
      if (seq !== loadSeq) return loaded.value
      providers.value = (all ?? []).filter((p) => !!speechCapabilityOf(p))
      loadError.value = ''
      loaded.value = true
      return true
    } catch (e) {
      if (seq !== loadSeq) return loaded.value
      loadError.value = String(e)
      // Keep the last good list: a failed refresh must not turn a working setup into "no providers".
      return false
    } finally {
      if (seq === loadSeq) loading.value = false
    }
  }

  // A provider added, removed, disabled or given a key changes what can speak.
  // The AI store already hears about all of those; this follows it.
  const providerSignature = computed(() =>
    (aiStore.settings.providers ?? [])
      .map((p) => `${p.id}:${p.kind}:${p.enabled ? 1 : 0}:${p.has_key ? 1 : 0}:${p.base_url}`)
      .join('|'),
  )
  watch(providerSignature, () => { void loadCapabilities() })

  // ── Configuration (stored in the app settings) ─────────────────────────────

  const providerId = computed(() => settingsStore.settings.speech_provider_id || '')
  const modelId = computed(() => settingsStore.settings.speech_model_id || '')
  const storedOptions = computed<Record<string, unknown>>(() => settingsStore.settings.speech_options ?? {})
  /** On unless explicitly switched off. */
  const skipCitations = computed(() => settingsStore.settings.speech_skip_citations !== false)

  const config = computed<SpeechConfig>(() => ({
    providerId: providerId.value,
    modelId: modelId.value,
    options: storedOptions.value,
    skipCitations: skipCitations.value,
  }))

  const selectedProvider = computed(() => providers.value.find((p) => p.providerId === providerId.value) ?? null)
  const selectedCapability = computed(() => (selectedProvider.value ? speechCapabilityOf(selectedProvider.value) ?? null : null))
  const selectedModel = computed<MediaModelSpec | null>(
    () => selectedCapability.value?.models.find((m) => m.id === modelId.value) ?? null,
  )

  /**
   * Why reading cannot start, or null when it can.
   *
   * - `no-providers`: nothing that can speak is set up at all (no provider with a
   *   speech adapter, enabled, with a key) — the fix is under AI 供应商.
   * - `unset`: there are options, none is chosen yet.
   * - `provider-missing`: one was chosen, and it is gone (deleted, disabled, key
   *   removed, or the model retired) — the choice must be made again.
   */
  const notConfiguredReason = computed<SpeechSetupReason | null>(() => {
    if (providers.value.length === 0) return 'no-providers'
    if (!providerId.value || !modelId.value) return 'unset'
    if (!selectedModel.value) return 'provider-missing'
    return null
  })
  const isConfigured = computed(() => notConfiguredReason.value === null)

  const maxChars = computed(() => selectedModel.value?.maxPromptChars ?? FALLBACK_MAX_CHARS)

  async function save(patch: Partial<SpeechConfig>): Promise<void> {
    const out: Partial<AppSettings> = {}
    if (patch.providerId !== undefined) out.speech_provider_id = patch.providerId
    if (patch.modelId !== undefined) out.speech_model_id = patch.modelId
    if (patch.options !== undefined) out.speech_options = patch.options
    if (patch.skipCitations !== undefined) out.speech_skip_citations = patch.skipCitations
    if (Object.keys(out).length) await settingsStore.save(out)
  }

  /** Choose a voice model. Its knobs restart from the model's own defaults, as the Media Studio does. */
  async function select(pid: string, mid: string): Promise<void> {
    const provider = providers.value.find((p) => p.providerId === pid)
    const model = provider ? speechCapabilityOf(provider)?.models.find((m) => m.id === mid) : undefined
    await save({ providerId: pid, modelId: mid, options: defaultOptions(model) })
  }

  async function setOption(key: string, value: unknown): Promise<void> {
    const next = { ...storedOptions.value }
    if (value === undefined || value === null || value === '') delete next[key]
    else next[key] = value
    await save({ options: next })
  }

  // ── Reading ────────────────────────────────────────────────────────────────

  const state = ref<SpeechState>('idle')
  const progress = ref({ index: 0, total: 0 })
  const errorMessage = ref('')
  const errorReason = ref<SpeechErrorReason | 'empty' | 'capabilities' | null>(null)
  const setupPrompt = ref<{ reason: SpeechSetupReason } | null>(null)
  /** Who started the current read (`opts.source`), for the UI. */
  const activeSource = ref('')
  const isReading = computed(() => state.value === 'loading' || state.value === 'playing' || state.value === 'paused')

  /**
   * The request settings of every read still alive, by scope. The engine hands
   * the scope back with each request, so a retry or late prefetch of an older
   * read keeps the voice it was started with even after the user changed it.
   */
  const scopeConfigs = new Map<string, RequestConfig>()
  const MAX_SCOPES = 16

  /** Identity of the read in progress (source + text), so the same click again toggles it off. A ref so a popup can show "stop" for exactly the text being read. */
  const currentKey = ref('')
  const keyOf = (text: string, source?: string) => `${source ?? ''}\u0001${text}`
  /** Bumped by every read() and stop(): an awaiting read that finds it moved on gives up. */
  let readSeq = 0

  async function synthesizeChunk(text: string, _signal: SynthSignal, scope: string): Promise<SynthResult> {
    const cfg = scopeConfigs.get(scope)
    if (!cfg) throw new Error('speech request settings are missing')
    const result = await invoke<MediaResult>('run_media_task', {
      request: {
        providerId: cfg.providerId,
        kind: 'speech',
        model: cfg.model,
        prompt: text,
        options: cfg.options,
      },
    })
    const artifacts = result?.artifacts ?? []
    const art = artifacts.find((a) => a.mime?.startsWith('audio/') && (a.dataUrl || a.url)) ?? artifacts.find((a) => a.dataUrl || a.url)
    if (!art) throw new Error('empty audio returned')
    const mime = art.mime || 'audio/mpeg'
    const dataUrl = art.dataUrl ?? (art.url ? await downloadAsDataUrl(art.url, mime) : '')
    return { dataUrl, mime }
  }

  function onEngineState(s: SpeechState, info: SpeechInfo) {
    state.value = s
    if (s === 'idle') {
      progress.value = { index: 0, total: 0 }
      errorMessage.value = ''
      errorReason.value = null
      activeSource.value = ''
      currentKey.value = ''
      return
    }
    progress.value = { index: info.index, total: info.total }
    if (s === 'error' && info.error) {
      errorReason.value = info.error.reason
      errorMessage.value = describeError(info.error.reason, info.error.message)
    } else {
      errorMessage.value = ''
      errorReason.value = null
    }
  }

  function describeError(reason: SpeechErrorReason, message: string): string {
    switch (reason) {
      case 'autoplay': return t('speech.errors.autoplay')
      case 'decode': return t('speech.errors.decode', { message })
      case 'playback': return t('speech.errors.playback', { message })
      default: return t('speech.errors.synth', { message })
    }
  }

  const engine = new SpeechEngine({
    synthesize: synthesizeChunk,
    createAudio: () => new Audio(),
    onState: onEngineState,
  })

  function setError(reason: 'empty' | 'capabilities', message: string) {
    engine.stop()
    state.value = 'error'
    errorReason.value = reason
    errorMessage.value = message
    progress.value = { index: 0, total: 0 }
  }

  /**
   * Read `text` aloud with the configured voice.
   *
   * Call it SYNCHRONOUSLY from the click handler — before any `await` — because
   * the first thing it does is unlock audio playback, which the WebView only
   * allows while the click is still on the stack (see `SpeechEngine.unlock`).
   * Everything after that may take as long as it likes.
   *
   * - Not configured: opens the setup prompt (`setupPrompt`) and never calls the
   *   backend, so nothing is billed.
   * - The same text while it is already being read: stops it (the popup button
   *   is a toggle).
   * - Any other text: replaces whatever was being read.
   *
   * Never rejects.
   */
  function read(text: string, opts?: { source?: string }): Promise<void> {
    engine.unlock()
    return readInner(text, opts).catch((e) => {
      console.error('Read aloud failed:', e)
      setError('capabilities', String(e))
    })
  }

  async function readInner(text: string, opts?: { source?: string }): Promise<void> {
    const key = keyOf(text, opts?.source)
    if (isReading.value && key === currentKey.value) {
      stop()
      return
    }
    const seq = ++readSeq
    setupPrompt.value = null

    if (!loaded.value) {
      const ok = await loadCapabilities()
      if (seq !== readSeq) return
      if (!ok && !loaded.value) {
        setError('capabilities', t('speech.errors.capabilities', { message: loadError.value }))
        return
      }
    }

    if (!isConfigured.value) {
      engine.stop()
      state.value = 'idle'
      setupPrompt.value = { reason: notConfiguredReason.value ?? 'unset' }
      return
    }

    const model = selectedModel.value as MediaModelSpec
    const prepared = prepareSpeechText(text, { skipCitations: skipCitations.value })
    if (!prepared) {
      setError('empty', t('speech.errors.empty'))
      return
    }
    const chunks = chunkForSpeech(prepared, { maxChars: model.maxPromptChars ?? FALLBACK_MAX_CHARS })
    const cfg: RequestConfig = {
      providerId: providerId.value,
      model: modelId.value,
      options: effectiveOptions(model, storedOptions.value),
    }
    const scope = speechScope(cfg.providerId, cfg.model, cfg.options)
    if (scopeConfigs.size >= MAX_SCOPES && !scopeConfigs.has(scope)) {
      const oldest = scopeConfigs.keys().next().value
      if (oldest !== undefined) scopeConfigs.delete(oldest)
    }
    scopeConfigs.set(scope, cfg)

    currentKey.value = key
    activeSource.value = opts?.source ?? ''
    engine.read(chunks, scope)
  }

  /** Stop reading and clear any error. */
  function stop() {
    readSeq++
    engine.stop()
    if (state.value !== 'idle') {
      // `engine.stop()` is silent when the engine itself was idle (an error that
      // came from this store, not from playback).
      state.value = 'idle'
      errorMessage.value = ''
      errorReason.value = null
      progress.value = { index: 0, total: 0 }
    }
    currentKey.value = ''
    activeSource.value = ''
  }

  /**
   * Whether THIS selection is the one being read right now — what a popup button
   * checks to show "stop" instead of "read aloud". Reactive, so it can sit in a
   * template. Another selection (or another source) while something is playing is
   * `false`: pressing the button then starts that one instead.
   */
  function isReadingText(text: string, source?: string): boolean {
    return isReading.value && currentKey.value === keyOf(text, source)
  }

  function pause() { engine.pause() }
  function resume() { engine.resume() }
  function togglePause() { if (state.value === 'playing') pause(); else if (state.value === 'paused') resume() }

  /** Speak a short bilingual sample with the current voice. Billed per character like any other read. */
  function preview(): Promise<void> {
    return read(PREVIEW_TEXT, { source: 'preview' })
  }

  function dismissSetup() { setupPrompt.value = null }

  /** Open 设置 at a section (`'speech'` = AI 随航 → 朗读, `'ai'` = AI 供应商). MainView owns the modal. */
  function openSettings(section: 'speech' | 'ai' = 'speech') {
    setupPrompt.value = null
    window.dispatchEvent(new CustomEvent('argus-open-settings', { detail: { section } }))
  }

  /** Load the capability list once; call from a mounted host. */
  function init() { if (!loaded.value && !loading.value) void loadCapabilities() }

  return {
    // capabilities
    providers, loaded, loading, loadError, loadCapabilities, init,
    // configuration
    config, providerId, modelId, skipCitations,
    selectedProvider, selectedCapability, selectedModel,
    isConfigured, notConfiguredReason, maxChars,
    save, select, setOption,
    // reading
    state, progress, errorMessage, errorReason, setupPrompt, activeSource, isReading,
    read, stop, pause, resume, togglePause, preview, isReadingText, dismissSetup, openSettings,
  }
})
