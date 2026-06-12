import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Canvas, CanvasIndexEntry, CanvasSettings } from '../types'

/** Editable snapshot of the currently selected canvas node, shared between the
 *  canvas (CanvasPanel) and the right-sidebar properties panel (DrawTab). */
export interface DrawNodeSnapshot {
  nodeId: string
  type: 'paper' | 'text' | 'shape' | 'line'
  lineKind?: 'line' | 'arrow'
  x: number
  y: number
  width?: number
  height?: number
  rotation?: number
  opacity?: number
  cornerRadius?: number
  /** Stroke/border (shape) or text color (text) or accent (paper). */
  color?: string
  fillColor?: string
  strokeWidth?: number
  shapeKind?: 'rect' | 'ellipse' | 'diamond'
  content?: string
  fontFamily?: string
  fontSize?: number
  bold?: boolean
  italic?: boolean
  textAlign?: 'left' | 'center' | 'right'
}

export const useCanvasStore = defineStore('canvas', () => {
  const canvasList = ref<CanvasIndexEntry[]>([])
  const currentCanvas = ref<Canvas | null>(null)
  const isShown = ref(false)      // whether canvas panel is currently in the center pane
  const settings = ref<CanvasSettings>({ hover_content_source: 'notes' })
  const loading = ref(false)
  const settingsSaving = ref(false)
  const settingsSaved = ref(false)

  // ── Drawing selection (canvas ↔ properties panel) ──────────────────────────
  const selectedNode = ref<DrawNodeSnapshot | null>(null)
  // All currently selected node ids (multi-select batch operations).
  const selectedNodeIds = ref<string[]>([])
  // CanvasPanel watches this and applies the patch to the live Vue Flow node.
  const pendingPatch = ref<{ nodeId: string; patch: Partial<DrawNodeSnapshot>; seq: number } | null>(null)
  // Generic action channel for batch ops (align/distribute/z-order/duplicate/…).
  const pendingAction = ref<{ type: string; payload?: unknown; seq: number } | null>(null)
  let patchSeq = 0
  let actionSeq = 0

  /** Called by CanvasPanel to publish the selected node's current properties. */
  function setSelectedNode(snap: DrawNodeSnapshot | null) {
    selectedNode.value = snap
  }

  function setSelectedNodeIds(ids: string[]) {
    selectedNodeIds.value = ids
  }

  /** Called by DrawTab to push a property change down to the canvas. */
  function patchNode(nodeId: string, patch: Partial<DrawNodeSnapshot>) {
    pendingPatch.value = { nodeId, patch, seq: ++patchSeq }
    // Optimistically reflect into the snapshot so inputs stay in sync.
    if (selectedNode.value && selectedNode.value.nodeId === nodeId) {
      selectedNode.value = { ...selectedNode.value, ...patch }
    }
  }

  /** Called by DrawTab to request a canvas-level action on the selection. */
  function requestAction(type: string, payload?: unknown) {
    pendingAction.value = { type, payload, seq: ++actionSeq }
  }

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
      isShown.value = false
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

  async function closeCurrentCanvas() {
    if (saveTimer) {
      clearTimeout(saveTimer)
      saveTimer = null
    }
    await persistCanvas()
    currentCanvas.value = null
    isShown.value = false
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
    isShown,
    settings,
    loading,
    settingsSaving,
    settingsSaved,
    selectedNode,
    selectedNodeIds,
    pendingPatch,
    pendingAction,
    setSelectedNode,
    setSelectedNodeIds,
    patchNode,
    requestAction,
    loadList,
    createCanvas,
    openCanvas,
    renameCanvas,
    deleteCanvas,
    scheduleSave,
    persistCanvas,
    closeCurrentCanvas,
    getNodeDisplayContent,
    loadSettings,
    saveSettings,
  }
})
