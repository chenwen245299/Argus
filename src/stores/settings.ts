import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { AppSettings, ThemeId } from '../types'

export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<AppSettings>({
    appearance: 'system',
    extraction_default: 'lopdf',
    usd_to_cny_rate: 7.2,
    metadata_ai_prompt: '',
    ai_summary_prompt: '',
  })
  const loaded = ref(false)

  async function load() {
    try {
      settings.value = await invoke<AppSettings>('get_settings')
      loaded.value = true
      applyAppearance(settings.value.appearance)
    } catch (e) {
      console.error('Failed to load settings:', e)
    }
  }

  async function save(patch: Partial<AppSettings>) {
    const previous = settings.value
    settings.value = { ...settings.value, ...patch }
    try {
      await invoke('save_settings', { settingsData: settings.value })
      applyAppearance(settings.value.appearance)
    } catch (e) {
      // Roll back the optimistic update so UI reflects the persisted state.
      settings.value = previous
      applyAppearance(settings.value.appearance)
      console.error('Failed to save settings:', e)
    }
  }

  function applyAppearance(mode: ThemeId | string) {
    const root = document.documentElement
    if (!mode || mode === 'system') {
      root.removeAttribute('data-theme')
    } else {
      root.setAttribute('data-theme', mode)
    }
  }

  return { settings, loaded, load, save }
})
