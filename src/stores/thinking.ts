import { defineStore } from 'pinia'
import { onScopeDispose, ref, watch } from 'vue'

interface ThinkingPreference {
  enabled: boolean
  level: string
}

// A per-device preference shared by the paper AI panel/window and library chat.
// Opening a different paper or conversation must not turn thinking off.
const STORAGE_KEY = 'argus:thinking-preference'

function parsePreference(raw: string | null): ThinkingPreference {
  try {
    const value = raw ? JSON.parse(raw) : null
    return {
      enabled: value?.enabled === true,
      level: typeof value?.level === 'string' && /^[a-z][a-z0-9_-]{0,31}$/.test(value.level)
        ? value.level : 'high',
    }
  } catch {
    return { enabled: false, level: 'high' }
  }
}

function loadPreference(): ThinkingPreference {
  try { return parsePreference(localStorage.getItem(STORAGE_KEY)) }
  catch { return parsePreference(null) }
}

export const useThinkingStore = defineStore('thinking', () => {
  const saved = loadPreference()
  const useReasoning = ref(saved.enabled)
  const reasoningLevel = ref(saved.level)
  let applyingStoredPreference = false

  watch([useReasoning, reasoningLevel], ([enabled, level]) => {
    if (applyingStoredPreference) return
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify({ enabled, level })) }
    catch { /* Keep the choice in this window if storage is unavailable. */ }
  }, { flush: 'sync' })

  // Tauri windows have separate Pinia instances. Follow changes made in another
  // window without writing an intermediate enabled/level pair back to storage.
  function onStorage(event: StorageEvent) {
    if (event.key !== STORAGE_KEY) return
    const next = parsePreference(event.newValue)
    applyingStoredPreference = true
    try {
      useReasoning.value = next.enabled
      reasoningLevel.value = next.level
    } finally {
      applyingStoredPreference = false
    }
  }

  if (typeof window !== 'undefined') {
    window.addEventListener('storage', onStorage)
    onScopeDispose(() => window.removeEventListener('storage', onStorage))
  }

  return { useReasoning, reasoningLevel }
})
