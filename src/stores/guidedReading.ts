import { ref } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { BUILTIN_GUIDED_READING_ID, type GuidedReadingSettings } from '../types/guidedReading'

const empty = (): GuidedReadingSettings => ({ presets: [], defaultId: BUILTIN_GUIDED_READING_ID })

export const useGuidedReadingStore = defineStore('guided-reading', () => {
  const settings = ref<GuidedReadingSettings>(empty())
  const root = ref<string | null>(null)
  let revision = 0

  async function load(): Promise<boolean> {
    const current = ++revision
    const library = await invoke<string | null>('get_current_library')
    if (current !== revision) return false
    if (library !== root.value) { root.value = library; settings.value = empty() }
    if (!library) return false
    const data = await invoke<GuidedReadingSettings>('get_guided_reading_settings', { expectedRoot: library })
    if (current !== revision) return false
    const latestRoot = await invoke<string | null>('get_current_library')
    if (current !== revision) return false
    if (latestRoot !== library) { root.value = latestRoot; settings.value = empty(); return false }
    // Missing settings in older libraries use the built-in guide.
    settings.value = data ?? empty()
    return true
  }

  async function save(data: GuidedReadingSettings, expectedRoot: string) {
    const library = await invoke<string | null>('get_current_library')
    if (library !== expectedRoot) throw new Error('Library changed')
    const snapshot = JSON.parse(JSON.stringify(data)) as GuidedReadingSettings
    await invoke('save_guided_reading_settings', { settingsData: snapshot, expectedRoot })
    if (root.value === expectedRoot) settings.value = snapshot
  }

  return { settings, root, load, save }
})
