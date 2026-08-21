import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  AiModel, AiProviderInfo, AiProviderInput, AiSettingsInfo, BalanceResult,
  ModelSelection, ProviderBalance,
} from '../types'

export interface ModelOption {
  providerId: string
  providerName: string
  modelId: string
  displayName: string
  contextLength?: number
  capabilities: string[]
  label: string  // "ProviderName / ModelName"
  // Price-list facts, carried so every picker can badge a model without
  // reaching back into the raw provider settings.
  paramBillions?: number
  isFree?: boolean
  discountPercent?: number
  discountWindows?: [number, number][]
}

/**
 * Whether a model can be sent images. Capability strings vary by provider —
 * OpenRouter reports modalities, Ollama reports `vision`, and the Qwen/DeepSeek
 * lists are derived from the model id — so the match is deliberately loose.
 */
export function modelHasVision(model: Pick<ModelOption, 'capabilities'> | null | undefined): boolean {
  return !!model?.capabilities?.some(c => /vision|image|multimodal/i.test(c))
}

/**
 * Whether a provider publishes an account balance. Mirrors `supports_balance`
 * in `src-tauri/src/balance.rs` — kept here only so a picker knows whether to
 * leave room for a balance while the lookup is still in flight; the backend
 * remains the authority on which providers are actually asked.
 */
export function providerSupportsBalance(
  provider: Pick<AiProviderInfo, 'kind' | 'base_url'> | null | undefined
): boolean {
  if (!provider) return false
  const url = provider.base_url.toLowerCase()
  return url.includes('deepseek') || provider.kind === 'openrouter' || url.includes('openrouter')
}

export const useAiStore = defineStore('ai', () => {
  const settings = ref<AiSettingsInfo>({ providers: [] })
  const loaded = ref(false)

  // The backend re-reads model prices in the background a while after launch.
  // Reloading here is what makes a withdrawn free tier stop showing FREE in an
  // already-open window, rather than waiting for the next restart. Registered
  // once per window, for as long as the window lives.
  listen('ai-models-refreshed', () => {
    if (loaded.value) load()
  }).catch(() => {})

  async function load() {
    try {
      settings.value = await invoke<AiSettingsInfo>('get_ai_settings')
      loaded.value = true
      // Warm the balances here rather than in each caller: every window that
      // shows a model picker loads settings first, so this is what makes the
      // figure already be there the first time a picker is opened. Not awaited
      // — settings must not wait on two network round-trips — and a no-op while
      // the last answer is still fresh.
      void loadBalances()
    } catch (e) {
      console.error('Failed to load AI settings:', e)
    }
  }

  const enabledModels = computed<ModelOption[]>(() =>
    settings.value.providers
      .filter(p => p.enabled)
      .flatMap(p =>
        p.models
          .map(m => ({
            providerId: p.id,
            providerName: p.name,
            modelId: m.id,
            displayName: m.display_name,
            contextLength: m.context_length,
            capabilities: m.capabilities,
            label: `${p.name} / ${m.display_name}`,
            paramBillions: m.param_billions,
            isFree: m.is_free,
            discountPercent: m.discount_percent,
            discountWindows: m.discount_windows,
          }))
      )
  )

  // Chat-capable models: exclude models that don't take a chat turn — pure
  // embedding models (vectors only) and pure media-generation models (image/video
  // out, labelled for the catalogue but not driven as chat here).
  const NON_CHAT_CAPS = ['embedding', 'image_gen', 'video']
  const chatModels = computed<ModelOption[]>(() =>
    enabledModels.value.filter(m => !m.capabilities.some(c => NON_CHAT_CAPS.includes(c)))
  )

  const groupedModels = computed(() => {
    const groups: Record<string, { id: string; name: string; models: ModelOption[] }> = {}
    for (const opt of chatModels.value) {
      if (!groups[opt.providerId]) {
        groups[opt.providerId] = { id: opt.providerId, name: opt.providerName, models: [] }
      }
      groups[opt.providerId].models.push(opt)
    }
    return Object.values(groups)
  })

  // ── Account balances ────────────────────────────────────────────────────────
  //
  // Only DeepSeek and OpenRouter publish one. The model pickers ask for them
  // when they open, which is often, so a looked-up balance is reused for a
  // short while rather than re-fetched on every click — the number only moves
  // when the user spends or tops up.

  const balances = ref<Record<string, ProviderBalance>>({})
  const balanceErrors = ref<Record<string, string>>({})
  const balancesLoading = ref(false)
  /** Earliest time an unforced refresh is allowed to go out again. */
  let balancesRetryAfter = 0

  /** How long a looked-up balance is reused before it is asked for again. */
  const BALANCE_FRESH_MS = 2 * 60 * 1000
  /**
   * Backoff after the *command* fails — no library open yet, mid-switch. Short,
   * because it usually clears on its own, but not zero: without it every picker
   * open and every settings load would re-fire a call that cannot succeed.
   */
  const BALANCE_RETRY_MS = 30 * 1000

  /**
   * Refresh every provider balance.
   *
   * Cheap to call from anywhere: it returns immediately while the last answer is
   * still fresh and never has more than one request in flight, which is what
   * lets the model pickers, the settings page and startup all just ask. A
   * per-provider failure (an expired key) is kept against that provider so it
   * does not blank out the others; a whole-command failure only sets the shorter
   * backoff.
   *
   * `force` skips the freshness window — used after a key changes, where the
   * whole point is that the previous answer is now wrong.
   */
  async function loadBalances(force = false) {
    if (balancesLoading.value) return
    if (!force && Date.now() < balancesRetryAfter) return
    balancesLoading.value = true
    try {
      const results = await invoke<BalanceResult[]>('fetch_provider_balances')
      const nextBalances: Record<string, ProviderBalance> = {}
      const nextErrors: Record<string, string> = {}
      for (const r of results) {
        if (r.balance) nextBalances[r.providerId] = r.balance
        else if (r.error) nextErrors[r.providerId] = r.error
      }
      balances.value = nextBalances
      balanceErrors.value = nextErrors
      balancesRetryAfter = Date.now() + BALANCE_FRESH_MS
    } catch {
      // The command itself failing (no library open, say) is not worth a
      // message in the picker — the tags simply stay hidden.
      balancesRetryAfter = Date.now() + BALANCE_RETRY_MS
    } finally {
      balancesLoading.value = false
    }
  }

  const defaultSelection = computed<ModelSelection | null>(() => {
    const pid = settings.value.default_provider_id
    const mid = settings.value.default_model_id
    if (!pid || !mid) return null
    return { providerId: pid, modelId: mid }
  })

  const isConfigured = computed(() => chatModels.value.length > 0)

  function findModel(sel: ModelSelection | null): ModelOption | null {
    if (!sel) return null
    return enabledModels.value.find(
      m => m.providerId === sel.providerId && m.modelId === sel.modelId
    ) ?? null
  }

  // ── CRUD helpers ─────────────────────────────────────────────────────────────

  async function addProvider(input: AiProviderInput, apiKey: string): Promise<AiProviderInfo> {
    const p = await invoke<AiProviderInfo>('add_ai_provider', { provider: input, apiKey })
    await load()
    // A brand-new key: whatever is cached cannot describe it.
    void loadBalances(true)
    return p
  }

  async function updateProvider(input: AiProviderInput, apiKey?: string) {
    await invoke('update_ai_provider', { provider: input, apiKey: apiKey ?? null })
    await load()
    // Only when the key actually changed — renaming a provider or toggling its
    // tools leaves the balance exactly as it was, and forcing there would turn
    // every blur of the name field into a pair of network calls.
    if (apiKey) void loadBalances(true)
  }

  async function deleteProvider(id: string) {
    await invoke('delete_ai_provider', { id })
    await load()
  }

  async function setProviderEnabled(id: string, enabled: boolean) {
    await invoke('set_provider_enabled', { id, enabled })
    const p = settings.value.providers.find(x => x.id === id)
    if (p) p.enabled = enabled
  }

  async function fetchModels(id: string): Promise<AiModel[]> {
    return invoke<AiModel[]>('fetch_provider_models', { id })
  }

  async function saveModels(id: string, models: AiModel[]) {
    await invoke('save_provider_models', { id, models })
    const p = settings.value.providers.find(x => x.id === id)
    if (p) p.models = models
  }

  async function setDefault(sel: ModelSelection) {
    await invoke('set_default_model', { providerId: sel.providerId, modelId: sel.modelId })
    settings.value.default_provider_id = sel.providerId
    settings.value.default_model_id = sel.modelId
  }

  async function testProvider(id: string): Promise<string> {
    return invoke<string>('test_ai_provider', { id })
  }

  return {
    settings,
    loaded,
    load,
    enabledModels,
    chatModels,
    groupedModels,
    balances,
    balanceErrors,
    balancesLoading,
    loadBalances,
    defaultSelection,
    isConfigured,
    findModel,
    addProvider,
    updateProvider,
    deleteProvider,
    setProviderEnabled,
    fetchModels,
    saveModels,
    setDefault,
    testProvider,
  }
})
