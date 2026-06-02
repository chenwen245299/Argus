import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { AiModel, AiProviderInfo, AiProviderInput, AiSettingsInfo, ModelSelection } from '../types'

export interface ModelOption {
  providerId: string
  providerName: string
  modelId: string
  displayName: string
  contextLength?: number
  capabilities: string[]
  label: string  // "ProviderName / ModelName"
}

export const useAiStore = defineStore('ai', () => {
  const settings = ref<AiSettingsInfo>({ providers: [] })
  const loaded = ref(false)

  async function load() {
    try {
      settings.value = await invoke<AiSettingsInfo>('get_ai_settings')
      loaded.value = true
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
          }))
      )
  )

  // Chat-capable models: exclude pure embedding models (they only produce vectors, not text)
  const chatModels = computed<ModelOption[]>(() =>
    enabledModels.value.filter(m => !m.capabilities.includes('embedding'))
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
    return p
  }

  async function updateProvider(input: AiProviderInput, apiKey?: string) {
    await invoke('update_ai_provider', { provider: input, apiKey: apiKey ?? null })
    await load()
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
