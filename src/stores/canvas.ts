import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Canvas, CanvasIndexEntry, CanvasSettings } from '../types'

export const useCanvasStore = defineStore('canvas', () => {
  const canvasList = ref<CanvasIndexEntry[]>([])
  const currentCanvas = ref<Canvas | null>(null)
  const settings = ref<CanvasSettings>({ hover_content_source: 'notes' })
  const loading = ref(false)
  const settingsSaving = ref(false)
  const settingsSaved = ref(false)

  // Debounce timer for auto-save
  let saveTimer: ReturnType<typeof setTimeout> | null = null

  async function loadList() {
    try {
      canvasList.value = await invoke<CanvasIndexEntry[]>('list_canvases')
    } catch (e) {
      console.error('list_canvases:', e)
    }
  }

  async function createCanvas(name: string): Promise<Canvas> {
    const canvas = await invoke<Canvas>('create_canvas', { name })
    await loadList()
    return canvas
  }

  async function openCanvas(id: string) {
    loading.value = true
    try {
      currentCanvas.value = await invoke<Canvas>('get_canvas', { id })
    } finally {
      loading.value = false
    }
  }

  async function renameCanvas(id: string, newName: string) {
    await invoke('rename_canvas', { id, newName })
    await loadList()
    if (currentCanvas.value?.id === id) {
      currentCanvas.value = { ...currentCanvas.value, name: newName }
    }
  }

  async function deleteCanvas(id: string) {
    await invoke('delete_canvas', { id })
    if (currentCanvas.value?.id === id) {
      currentCanvas.value = null
    }
    await loadList()
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => persistCanvas(), 800)
  }

  async function persistCanvas() {
    if (!currentCanvas.value) return
    try {
      await invoke('save_canvas', { canvasData: currentCanvas.value })
      // Refresh the index entry
      const idx = canvasList.value.findIndex(e => e.id === currentCanvas.value!.id)
      if (idx >= 0) {
        canvasList.value[idx] = {
          ...canvasList.value[idx],
          node_count: currentCanvas.value.nodes.length,
          updated_at: currentCanvas.value.updated_at,
        }
      }
    } catch (e) {
      console.error('save_canvas:', e)
    }
  }

  async function getNodeDisplayContent(paperId: string, source: string): Promise<string> {
    try {
      return await invoke<string>('get_node_display_content', { paperId, source })
    } catch {
      return ''
    }
  }

  async function loadSettings() {
    try {
      settings.value = await invoke<CanvasSettings>('get_canvas_settings')
    } catch (e) {
      console.error('get_canvas_settings:', e)
    }
  }

  async function saveSettings() {
    settingsSaving.value = true
    try {
      await invoke('save_canvas_settings', { settings: settings.value })
      settingsSaved.value = true
      setTimeout(() => (settingsSaved.value = false), 2000)
    } finally {
      settingsSaving.value = false
    }
  }

  return {
    canvasList,
    currentCanvas,
    settings,
    loading,
    settingsSaving,
    settingsSaved,
    loadList,
    createCanvas,
    openCanvas,
    renameCanvas,
    deleteCanvas,
    scheduleSave,
    persistCanvas,
    getNodeDisplayContent,
    loadSettings,
    saveSettings,
  }
})
