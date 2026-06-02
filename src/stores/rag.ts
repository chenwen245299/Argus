import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { RagSettings, VectorStoreInfo } from '../types'

export const useRagStore = defineStore('rag', () => {
  const settings = ref<RagSettings>({
    provider_id: null,
    embedding_model: null,
    chunk_size: 800,
    chunk_overlap: 100,
    top_k: 5,
    enabled: true,
  })
  const storeInfo = ref<VectorStoreInfo | null>(null)
  const loaded = ref(false)

  const isConfigured = computed(
    () =>
      settings.value.enabled &&
      !!settings.value.provider_id &&
      settings.value.provider_id.length > 0 &&
      !!settings.value.embedding_model &&
      settings.value.embedding_model.length > 0
  )

  async function load() {
    try {
      settings.value = await invoke<RagSettings>('get_rag_settings')
      loaded.value = true
    } catch { /* no library open */ }
  }

  async function save(s: RagSettings) {
    await invoke('save_rag_settings', { settings: s })
    settings.value = s
  }

  async function loadStoreInfo() {
    try {
      storeInfo.value = await invoke<VectorStoreInfo>('get_vector_store_info')
    } catch { storeInfo.value = null }
  }

  return { settings, storeInfo, loaded, isConfigured, load, save, loadStoreInfo }
})
