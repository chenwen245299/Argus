import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Canvas, CanvasEditOp, CanvasIndexEntry, CanvasSettings } from '../types'

/** An AI canvas edit as it travels from the canvas chat (which owns the
 *  backend approval handshake) to the CanvasPanel (which owns the live document
 *  and draws/applies the change). One request at a time, keyed by `requestId`:
 *
 *   - `preview`  — the panel draws the change on the canvas, awaiting a decision
 *   - `apply`    — the user approved; the panel applies these same ops and saves
 *   - `discard`  — declined/timed out/left; the panel just clears the preview
 *
 *  `seq` bumps on every phase change so the panel's watcher fires even when the
 *  request id is unchanged, matching how pendingPatch/pendingAction work. */
export interface CanvasEditRequest {
  requestId: string
  canvasId: string
  ops: CanvasEditOp[]
  phase: 'preview' | 'apply' | 'discard'
  seq: number
}

/** Editable snapshot of the currently selected canvas node, shared between the
 *  canvas (CanvasPanel) and the right-sidebar properties panel (DrawTab). */
export interface DrawNodeSnapshot {
  nodeId: string
  type: 'paper' | 'text' | 'shape' | 'line' | 'image'
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
  imageSrc?: string
  imageAlt?: string
}

export const useCanvasStore = defineStore('canvas', () => {
  const canvasList = ref<CanvasIndexEntry[]>([])

  // Several canvases can be open as tabs at once, each with its own live
  // document, so loaded canvases are kept by id rather than one at a time. Every
  // CanvasPanel instance reads ITS canvas by id; the store just holds the data.
  const canvasesById = ref<Record<string, Canvas>>({})
  const activeCanvasId = ref<string | null>(null)
  const isShown = ref(false)      // whether a canvas panel is currently in the center pane

  function canvasById(id: string | null | undefined): Canvas | null {
    return id ? canvasesById.value[id] ?? null : null
  }

  function setCanvas(id: string, canvas: Canvas) {
    canvasesById.value = { ...canvasesById.value, [id]: canvas }
  }

  /** The active canvas. Writable so the single-canvas callers (CanvasView, and
   *  CanvasPanel's own doc updates) keep working unchanged. */
  const currentCanvas = computed<Canvas | null>({
    get: () => canvasById(activeCanvasId.value),
    set: (value) => {
      const id = activeCanvasId.value
      if (!id) return
      if (value) setCanvas(id, value)
      else dropCanvas(id)
    },
  })

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
  // An AI edit the canvas chat is proposing to this canvas (see CanvasEditRequest).
  const canvasEditRequest = ref<CanvasEditRequest | null>(null)
  let patchSeq = 0
  let actionSeq = 0
  let canvasEditSeq = 0

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

  // ── AI canvas edits (canvas chat ↔ CanvasPanel) ────────────────────────────

  /** The canvas chat asks the matching panel to preview an AI edit. */
  function proposeCanvasEdit(requestId: string, canvasId: string, ops: CanvasEditOp[]) {
    canvasEditRequest.value = { requestId, canvasId, ops, phase: 'preview', seq: ++canvasEditSeq }
  }

  /** The user answered the approval card: apply the preview or drop it. Ignored
   *  if it does not name the request currently on screen (a stale click). */
  function resolveCanvasEdit(requestId: string, action: 'apply' | 'discard') {
    const cur = canvasEditRequest.value
    if (!cur || cur.requestId !== requestId) return
    canvasEditRequest.value = { ...cur, phase: action, seq: ++canvasEditSeq }
  }

  /** Drop the pending edit entirely (nothing to preview any more). */
  function clearCanvasEdit(requestId?: string) {
    const cur = canvasEditRequest.value
    if (!cur) return
    if (requestId && cur.requestId !== requestId) return
    canvasEditRequest.value = null
  }

  // Auto-save debounce, one timer per open canvas — a shared timer would let a
  // keystroke in one canvas postpone another canvas's pending save.
  const saveTimers = new Map<string, ReturnType<typeof setTimeout>>()
  // Timer that resets the "saved" flash indicator.
  let settingsSavedTimer: ReturnType<typeof setTimeout> | null = null

  function cancelSave(id: string) {
    const timer = saveTimers.get(id)
    if (timer) { clearTimeout(timer); saveTimers.delete(id) }
  }

  /** Forget a canvas's loaded document, cancelling any save still pending. */
  function dropCanvas(id: string) {
    cancelSave(id)
    if (id in canvasesById.value) {
      const next = { ...canvasesById.value }
      delete next[id]
      canvasesById.value = next
    }
    if (activeCanvasId.value === id) activeCanvasId.value = null
  }

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

  /** Load a canvas (if not already loaded) and make it the active one. */
  async function openCanvas(id: string) {
    activeCanvasId.value = id
    if (canvasesById.value[id]) return   // already open in another tab
    loading.value = true
    try {
      setCanvas(id, await invoke<Canvas>('get_canvas', { id }))
    } finally {
      loading.value = false
    }
  }

  async function renameCanvas(id: string, newName: string) {
    await invoke('rename_canvas', { id, newName })
    await loadList()
    const open = canvasById(id)
    if (open) setCanvas(id, { ...open, name: newName })
  }

  async function deleteCanvas(id: string) {
    await invoke('delete_canvas', { id })
    dropCanvas(id)
    if (!activeCanvasId.value) isShown.value = false
    await loadList()
  }

  /** `id` defaults to the active canvas, for the single-canvas callers. */
  function scheduleSave(id?: string) {
    const key = id ?? activeCanvasId.value
    if (!key) return
    cancelSave(key)
    saveTimers.set(key, setTimeout(() => {
      saveTimers.delete(key)
      void persistCanvas(key)
    }, 800))
  }

  async function persistCanvas(id?: string) {
    const key = id ?? activeCanvasId.value
    const canvas = canvasById(key)
    if (!key || !canvas) return
    try {
      await invoke('save_canvas', { canvasData: canvas })
      // Refresh the index entry
      const idx = canvasList.value.findIndex(e => e.id === key)
      if (idx >= 0) {
        canvasList.value[idx] = {
          ...canvasList.value[idx],
          node_count: canvas.nodes.length,
          updated_at: canvas.updated_at,
        }
      }
    } catch (e) {
      console.error('save_canvas:', e)
    }
  }

  /** Flush a canvas's pending edits and unload it (its tab is closing). */
  async function closeCanvas(id: string) {
    cancelSave(id)
    await persistCanvas(id)
    dropCanvas(id)
    if (!activeCanvasId.value) isShown.value = false
  }

  async function closeCurrentCanvas() {
    if (activeCanvasId.value) await closeCanvas(activeCanvasId.value)
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
      // Clear any pending reset so rapid re-saves don't cause the indicator to
      // flicker off early.
      if (settingsSavedTimer) clearTimeout(settingsSavedTimer)
      settingsSavedTimer = setTimeout(() => {
        settingsSaved.value = false
        settingsSavedTimer = null
      }, 2000)
    } finally {
      settingsSaving.value = false
    }
  }

  return {
    canvasList,
    canvasesById,
    activeCanvasId,
    canvasById,
    setCanvas,
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
    canvasEditRequest,
    setSelectedNode,
    setSelectedNodeIds,
    patchNode,
    requestAction,
    proposeCanvasEdit,
    resolveCanvasEdit,
    clearCanvasEdit,
    loadList,
    createCanvas,
    openCanvas,
    renameCanvas,
    deleteCanvas,
    scheduleSave,
    persistCanvas,
    closeCanvas,
    closeCurrentCanvas,
    getNodeDisplayContent,
    loadSettings,
    saveSettings,
  }
})
