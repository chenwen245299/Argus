import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { AppSettings } from '../types'

export const useSettingsStore = defineStore('settings', () => {
  const settings = ref<AppSettings>({
    appearance: 'system',
    appearance_light: 'light',
    appearance_dark: 'dark',
    extraction_default: 'lopdf',
    usd_to_cny_rate: 7.2,
    metadata_ai_prompt: '',
    ai_summary_prompt: '',
    auto_check_updates: true,
  })
  const loaded = ref(false)
  // Whether an easyScholar secret key is stored (encrypted). The key value
  // itself never reaches the frontend.
  const easyscholarConfigured = ref(false)
  // Whether a Semantic Scholar API key is stored (encrypted). Same treatment:
  // the key value never reaches the frontend.
  const semanticScholarConfigured = ref(false)

  async function load() {
    try {
      settings.value = await invoke<AppSettings>('get_settings')
      loaded.value = true
      ensureSchemeListener()
      // No transition at startup — avoids a flash of animating colors on launch.
      applyAppearance(false)
    } catch (e) {
      console.error('Failed to load settings:', e)
    }
    await loadEasyscholarStatus()
    await loadSemanticScholarStatus()
  }

  async function loadEasyscholarStatus() {
    try {
      easyscholarConfigured.value = await invoke<boolean>('easyscholar_key_status')
    } catch {
      easyscholarConfigured.value = false
    }
  }

  async function setEasyscholarKey(key: string) {
    await invoke('set_easyscholar_key', { key })
    easyscholarConfigured.value = !!key.trim()
  }

  async function loadSemanticScholarStatus() {
    try {
      semanticScholarConfigured.value = await invoke<boolean>('semantic_scholar_key_status')
    } catch {
      semanticScholarConfigured.value = false
    }
  }

  async function setSemanticScholarKey(key: string) {
    await invoke('set_semantic_scholar_key', { key })
    semanticScholarConfigured.value = !!key.trim()
  }

  async function save(patch: Partial<AppSettings>) {
    const previous = settings.value
    settings.value = { ...settings.value, ...patch }
    try {
      await invoke('save_settings', { settingsData: settings.value })
      applyAppearance()
    } catch (e) {
      // Roll back the optimistic update so UI reflects the persisted state.
      settings.value = previous
      applyAppearance()
      console.error('Failed to save settings:', e)
    }
  }

  // Briefly enables color transitions on <html> while a theme is applied so
  // the UI fades into the new palette instead of snapping. The class is
  // always removed again — it must never stay on permanently.
  let themeAnimTimer: ReturnType<typeof setTimeout> | null = null
  let schemeMedia: MediaQueryList | null = null

  // Concrete palette that should be active right now. In 'system' mode this
  // follows the OS light/dark setting using the user's per-mode preferences.
  function resolveTheme(): string {
    const s = settings.value
    if (s.appearance && s.appearance !== 'system') return s.appearance
    const osDark = window.matchMedia('(prefers-color-scheme: dark)').matches
    return osDark ? (s.appearance_dark || 'dark') : (s.appearance_light || 'light')
  }

  // Re-apply the theme when the OS flips light/dark while in 'system' mode.
  function ensureSchemeListener() {
    if (schemeMedia) return
    schemeMedia = window.matchMedia('(prefers-color-scheme: dark)')
    schemeMedia.addEventListener('change', () => {
      if (settings.value.appearance === 'system') applyAppearance()
    })
  }

  function applyAppearance(animate = true) {
    const root = document.documentElement
    if (animate) {
      root.classList.add('theme-animating')
      if (themeAnimTimer) clearTimeout(themeAnimTimer)
      themeAnimTimer = setTimeout(() => {
        root.classList.remove('theme-animating')
        themeAnimTimer = null
      }, 320)
    }
    root.setAttribute('data-theme', resolveTheme())
  }

  return {
    settings,
    loaded,
    easyscholarConfigured,
    semanticScholarConfigured,
    load,
    save,
    loadEasyscholarStatus,
    setEasyscholarKey,
    loadSemanticScholarStatus,
    setSemanticScholarKey,
  }
})
