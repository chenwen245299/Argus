import { ref } from 'vue'
import type { CanvasNode as CNode, CanvasEdge as CEdge } from '../types'

// A single point-in-time snapshot of the canvas graph (nodes + edges), fully
// serialized to the plain CNode[]/CEdge[] shape produced by the existing
// extractCanvasNodes()/extractCanvasEdges() helpers. Capturing that shape means
// a snapshot covers every structural change: add/remove of nodes & edges, drag
// positions, colors, sizes, text edits, edge labels, control points, etc.
export interface CanvasSnapshot {
  nodes: CNode[]
  edges: CEdge[]
}

const MAX_HISTORY = 50

// Deep-clone a snapshot so mutations to the live graph never leak into history
// (and vice-versa). Prefer structuredClone, fall back to a JSON round-trip.
function cloneSnapshot(snap: CanvasSnapshot): CanvasSnapshot {
  try {
    return structuredClone(snap)
  } catch {
    return JSON.parse(JSON.stringify(snap)) as CanvasSnapshot
  }
}

export interface UseCanvasHistoryOptions {
  // Produce the current graph as a snapshot (from the live Vue Flow refs).
  capture: () => CanvasSnapshot
  // Rebuild the live Vue Flow refs from a snapshot (reverse of capture).
  restore: (snap: CanvasSnapshot) => void
  // Persist the canvas after an undo/redo (usually the existing triggerSave).
  persist: () => void
}

/**
 * Snapshot-based undo/redo history for the canvas.
 *
 * Model: a `present` snapshot plus two stacks. Every real change calls
 * `commit(newSnapshot)`, which pushes the *old* present onto undoStack, sets
 * present = new, and clears redoStack. `undo()` moves present onto redoStack and
 * pops undoStack back into present; `redo()` is the mirror. This avoids any
 * off-by-one: `present` always reflects what's currently on screen.
 *
 * While restoring, `isRestoring` is set so the restore-driven change events
 * (which would otherwise call commit) are ignored — preventing pollution and
 * infinite loops.
 */
export function useCanvasHistory(opts: UseCanvasHistoryOptions) {
  const undoStack = ref<CanvasSnapshot[]>([])
  const redoStack = ref<CanvasSnapshot[]>([])
  let present: CanvasSnapshot | null = null
  const isRestoring = ref(false)

  const canUndo = ref(false)
  const canRedo = ref(false)

  function syncFlags() {
    canUndo.value = undoStack.value.length > 0
    canRedo.value = redoStack.value.length > 0
  }

  // Establish the initial baseline (e.g. right after a canvas loads). This is
  // NOT an undoable step — it only seeds `present` and clears the stacks.
  function reset(snap?: CanvasSnapshot) {
    present = cloneSnapshot(snap ?? opts.capture())
    undoStack.value = []
    redoStack.value = []
    syncFlags()
  }

  // Record a real change. Captures the current live state, and if it differs
  // from `present`, pushes the old present to undoStack and adopts the new one.
  function commit() {
    if (isRestoring.value) return
    if (!present) { reset(); return }
    const next = cloneSnapshot(opts.capture())
    // Skip no-op commits (identical serialization) to avoid dead history entries.
    if (JSON.stringify(next) === JSON.stringify(present)) return
    undoStack.value.push(present)
    if (undoStack.value.length > MAX_HISTORY) undoStack.value.shift()
    present = next
    redoStack.value = []
    syncFlags()
  }

  function applySnapshot(snap: CanvasSnapshot) {
    isRestoring.value = true
    try {
      opts.restore(cloneSnapshot(snap))
      present = cloneSnapshot(snap)
    } finally {
      isRestoring.value = false
    }
    opts.persist()
  }

  function undo() {
    if (!undoStack.value.length || !present) return
    redoStack.value.push(present)
    const prev = undoStack.value.pop() as CanvasSnapshot
    present = prev
    applySnapshot(prev)
    syncFlags()
  }

  function redo() {
    if (!redoStack.value.length || !present) return
    undoStack.value.push(present)
    const next = redoStack.value.pop() as CanvasSnapshot
    present = next
    applySnapshot(next)
    syncFlags()
  }

  return {
    isRestoring,
    canUndo,
    canRedo,
    reset,
    commit,
    undo,
    redo,
  }
}
