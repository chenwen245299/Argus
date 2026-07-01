<script setup lang="ts">
import {
  ref, computed, markRaw, onMounted, onUnmounted, nextTick, watch,
} from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { emit as tauriEmit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  VueFlow,
  useVueFlow,
  MarkerType,
  ConnectionMode,
  type Node as VfNode,
  type Edge as VfEdge,
  type Connection,
  type NodeMouseEvent,
  type EdgeMouseEvent,
} from '@vue-flow/core'
import { Background } from '@vue-flow/background'
import { Controls } from '@vue-flow/controls'
import { MiniMap } from '@vue-flow/minimap'
import '@vue-flow/core/dist/style.css'
import '@vue-flow/core/dist/theme-default.css'
import '@vue-flow/controls/dist/style.css'
import '@vue-flow/minimap/dist/style.css'
import { renderMarkdown } from '../utils/renderMarkdown'
import { useCanvasStore } from '../stores/canvas'
import { useLibraryStore } from '../stores/library'
import { useCanvasHistory, type CanvasSnapshot } from '../composables/useCanvasHistory'
import { recordPaperAccess, sortPapersByRecentAccess } from '../utils/recentPapers'
import PaperNode from '../components/canvas/PaperNode.vue'
import AdjustableEdge from '../components/canvas/AdjustableEdge.vue'
import TextNode from '../components/canvas/TextNode.vue'
import ShapeNode from '../components/canvas/ShapeNode.vue'
import LineNode from '../components/canvas/LineNode.vue'
import SuggestPanel from '../components/canvas/SuggestPanel.vue'
import ExportDialog from '../components/canvas/ExportDialog.vue'
import type { PaperIndexEntry, CanvasNode as CNode, CanvasEdge as CEdge, Canvas, SuggestedEdge, NodePosition } from '../types'

const { t } = useI18n()
const canvasStore = useCanvasStore()
const library = useLibraryStore()

let unlistenWindowResize: UnlistenFn | null = null
let windowResizeTimer: ReturnType<typeof setTimeout> | null = null

async function saveWindowSize() {
  try {
    const appWindow = getCurrentWindow()
    const [physSize, sf] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()])
    await invoke('save_canvas_window_size', {
      width: physSize.width / sf,
      height: physSize.height / sf,
    })
  } catch {
    // Not running inside Tauri, or the window is closing.
  }
}

async function watchWindowSize() {
  try {
    const appWindow = getCurrentWindow()
    unlistenWindowResize = await appWindow.onResized(() => {
      if (windowResizeTimer) clearTimeout(windowResizeTimer)
      windowResizeTimer = setTimeout(() => {
        saveWindowSize()
      }, 500)
    })
  } catch {
    // Not running inside Tauri.
  }
}

// ── Vue Flow setup ────────────────────────────────────────────────────────────

const nodeTypes = markRaw({ paper: PaperNode, text: TextNode, shape: ShapeNode, line: LineNode })
const edgeTypes = markRaw({ adjustable: AdjustableEdge })
const PAN_ON_DRAG_BUTTONS = [1, 2]

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const nodes = ref<any[]>([])
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const edges = ref<any[]>([])

const {
  onConnect,
  onConnectStart,
  onConnectEnd,
  addEdges,
  addNodes,
  removeNodes,
  removeEdges,
  onNodeDrag,
  onNodeDragStop,
  onEdgesChange,
  onNodesChange,
  getViewport,
  setViewport,
  fitView,
} = useVueFlow()

// Unique id generators — random + index guards against collisions when several
// ids are minted within the same tick (e.g. accept-all suggestions).
function newNodeId(i = 0): string {
  return `node-${Date.now()}-${Math.floor(Math.random() * 1e6)}-${i}`
}
function newEdgeId(i = 0): string {
  return `e-${Date.now()}-${Math.floor(Math.random() * 1e6)}-${i}`
}

type ConnectStartInfo = { nodeId?: string; handleId: string | null }
const connectStart = ref<ConnectStartInfo | null>(null)

function handleSide(handleId?: string | null) {
  return handleId?.replace(/^(src|tgt)-/, '') || null
}

function sourceHandleFor(handleId?: string | null) {
  const side = handleSide(handleId)
  return side ? `src-${side}` : (handleId ?? undefined)
}

function targetHandleFor(handleId?: string | null) {
  const side = handleSide(handleId)
  return side ? `tgt-${side}` : (handleId ?? undefined)
}

function normalizeDirectedConnection(params: Connection): Connection {
  const start = connectStart.value
  if (!start?.nodeId) return params

  if (start.nodeId === params.source) {
    return {
      source: params.source,
      target: params.target,
      sourceHandle: sourceHandleFor(start.handleId ?? params.sourceHandle),
      targetHandle: targetHandleFor(params.targetHandle),
    }
  }

  if (start.nodeId === params.target) {
    return {
      source: params.target,
      target: params.source,
      sourceHandle: sourceHandleFor(start.handleId ?? params.targetHandle),
      targetHandle: targetHandleFor(params.sourceHandle),
    }
  }

  return params
}

// ── Sidebar state ─────────────────────────────────────────────────────────────

const renamingId = ref<string | null>(null)
const renamingValue = ref('')
const deleteConfirmId = ref<string | null>(null)
const newCanvasName = ref('')
const creatingCanvas = ref(false)

// ── Paper picker dialog ───────────────────────────────────────────────────────

const showPaperPicker = ref(false)
const pickerSearch = ref('')
const pickerError = ref('')

const filteredPapers = computed(() => {
  const addedIds = new Set(nodes.value.map(n => n.data.paperId))
  const q = pickerSearch.value.trim().toLowerCase()
  const base = sortPapersByRecentAccess(library.papers).filter(p => !addedIds.has(p.id))
  if (!q) return base
  return base.filter(p =>
    p.title.toLowerCase().includes(q) ||
    p.authors.some(a => a.toLowerCase().includes(q)) ||
    String(p.year ?? '').includes(q)
  )
})

// ── Hover tooltip ─────────────────────────────────────────────────────────────

const hoverNodeId = ref<string | null>(null)
const hoverContent = ref('')
const hoverLoading = ref(false)
const hoverPos = ref({ x: 0, y: 0 })
let hoverTimer: ReturnType<typeof setTimeout> | null = null

// ── Context menu ──────────────────────────────────────────────────────────────

const ctxMenu = ref<{
  show: boolean; x: number; y: number
  nodeId: string | null; edgeId: string | null
}>({ show: false, x: 0, y: 0, nodeId: null, edgeId: null })

// ── Edge label editor ─────────────────────────────────────────────────────────

const editingEdgeId = ref<string | null>(null)
const editingEdgeLabel = ref('')

// ── M10: Suggest panel ────────────────────────────────────────────────────────
const showSuggestPanel = ref(false)

// ── M10: Layout panel ────────────────────────────────────────────────────────
const showLayoutMenu = ref(false)
const layoutHistory = ref<{ node_id: string; x: number; y: number }[]>([])
const applyingLayout = ref(false)

// ── M10: Export dialog ────────────────────────────────────────────────────────
const showExportDialog = ref(false)
const flowContainerRef = ref<HTMLElement | null>(null)

// ── M10: paperNames map for SuggestPanel ─────────────────────────────────────
const paperNames = computed<Record<string, string>>(() => {
  const map: Record<string, string> = {}
  ;(nodes.value as any[]).forEach(n => {
    if (n.data?.paperId && n.data?.title) {
      map[n.data.paperId] = n.data.title
    }
  })
  return map
})

// ── Helpers: convert between canvas JSON ↔ Vue Flow ──────────────────────────

function paperById(paperId: string): PaperIndexEntry | undefined {
  return library.papers.find(p => p.id === paperId)
}

const noteTitlesMap = ref<Map<string, string[]>>(new Map())

async function loadNoteTitles(cnodes: CNode[]) {
  const results = await Promise.allSettled(
    cnodes.map(async cn => {
      const paper = paperById(cn.paper_id)
      if (!paper) return
      const titles = await invoke<string[]>('get_canvas_note_titles', { slug: paper.slug })
      if (titles.length > 0) noteTitlesMap.value.set(cn.paper_id, titles)
    })
  )
  // Patch existing nodes with fresh note titles
  nodes.value = nodes.value.map(n => ({
    ...n,
    data: { ...n.data, noteTitles: noteTitlesMap.value.get(n.data.paperId as string) ?? [] },
  }))
}

function buildVfNodes(cnodes: CNode[]): VfNode[] {
  return cnodes.map(cn => {
    const nt = cn.node_type ?? 'paper'
    if (nt === 'text') {
      return {
        id: cn.node_id,
        type: 'text',
        position: { x: cn.x, y: cn.y },
        zIndex: cn.z_index,
        data: {
          content: cn.content ?? '',
          color: cn.color,
          fontSize: cn.font_size,
          bold: cn.font_bold,
          italic: cn.font_italic,
          fontFamily: cn.font_family,
          textAlign: cn.text_align,
          rotation: cn.rotation,
          opacity: cn.opacity,
          nodeId: cn.node_id,
        },
      } satisfies VfNode
    }
    if (nt === 'shape') {
      return {
        id: cn.node_id,
        type: 'shape',
        position: { x: cn.x, y: cn.y },
        zIndex: cn.z_index,
        data: {
          content: cn.content ?? '',
          color: cn.color,
          fillColor: cn.fill_color,
          nodeId: cn.node_id,
          width: cn.width,
          height: cn.height,
          shapeKind: cn.shape_kind,
          strokeWidth: cn.stroke_width,
          cornerRadius: cn.corner_radius,
          rotation: cn.rotation,
          opacity: cn.opacity,
        },
      } satisfies VfNode
    }
    if (nt === 'line') {
      return {
        id: cn.node_id,
        type: 'line',
        position: { x: cn.x, y: cn.y },
        zIndex: cn.z_index,
        data: {
          nodeId: cn.node_id,
          lineKind: cn.line_kind ?? 'arrow',
          color: cn.color,
          strokeWidth: cn.stroke_width,
          width: cn.width,
          height: cn.height,
          x1: cn.line_points?.[0]?.x ?? 0,
          y1: cn.line_points?.[0]?.y ?? 0,
          x2: cn.line_points?.[1]?.x ?? (cn.width ?? 0),
          y2: cn.line_points?.[1]?.y ?? (cn.height ?? 0),
        },
      } satisfies VfNode
    }
    const paper = paperById(cn.paper_id)
    return {
      id: cn.node_id,
      type: 'paper',
      position: { x: cn.x, y: cn.y },
      zIndex: cn.z_index,
      data: {
        title: paper?.title ?? '???',
        authors: paper?.authors ?? [],
        year: paper?.year,
        venue: paper?.venue,
        paperId: cn.paper_id,
        nodeId: cn.node_id,
        valid: !!paper,
        color: cn.color,
        hoverSource: cn.hover_source,
        noteTitles: noteTitlesMap.value.get(cn.paper_id) ?? [],
      },
    } satisfies VfNode
  })
}

function buildVfEdges(cedges: CEdge[]): VfEdge[] {
  return cedges.map(ce => {
    const sw = ce.stroke_width ?? 1.8
    const style: { strokeWidth: number; stroke?: string } = { strokeWidth: sw }
    if (ce.color) style.stroke = ce.color
    const hasLegacyControlPoint = typeof ce.control_x === 'number' && Number.isFinite(ce.control_x) &&
      typeof ce.control_y === 'number' && Number.isFinite(ce.control_y)
    const controlPoints = ce.control_points?.length
      ? ce.control_points
      : (hasLegacyControlPoint ? [{ x: ce.control_x as number, y: ce.control_y as number }] : undefined)
    return {
      id: ce.edge_id,
      source: ce.from_node_id,
      target: ce.to_node_id,
      sourceHandle: ce.source_handle,
      targetHandle: ce.target_handle,
      label: ce.label,
      markerEnd: ce.color ? { type: MarkerType.ArrowClosed, color: ce.color } : MarkerType.ArrowClosed,
      style,
      labelStyle: { fontSize: '11px' },
      labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
      type: 'adjustable',
      data: {
        edgeColor: ce.color,
        edgeStrokeWidth: sw,
        controlPoints,
      },
    } satisfies VfEdge
  })
}

function extractCanvasNodes(): CNode[] {
  return (nodes.value as VfNode[]).map((n): CNode => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const d = n.data as any
    const nt = n.type as string
    if (nt === 'line') {
      return {
        node_id: n.id,
        paper_id: '',
        x: n.position.x,
        y: n.position.y,
        z_index: Number.isFinite(n.zIndex) ? (n.zIndex as number) : undefined,
        color: d.color as string | undefined,
        hover_source: undefined,
        node_type: 'line',
        width: Number.isFinite(d.width) ? d.width : undefined,
        height: Number.isFinite(d.height) ? d.height : undefined,
        stroke_width: Number.isFinite(d.strokeWidth) ? d.strokeWidth : undefined,
        line_kind: d.lineKind as 'line' | 'arrow' | undefined,
        line_points: [
          { x: (d.x1 as number) ?? 0, y: (d.y1 as number) ?? 0 },
          { x: (d.x2 as number) ?? 0, y: (d.y2 as number) ?? 0 },
        ],
      }
    }
    if (nt === 'text' || nt === 'shape') {
      return {
        node_id: n.id,
        paper_id: '',
        x: n.position.x,
        y: n.position.y,
        z_index: Number.isFinite(n.zIndex) ? (n.zIndex as number) : undefined,
        color: d.color as string | undefined,
        hover_source: undefined,
        node_type: nt,
        content: d.content as string | undefined,
        font_size: nt === 'text' ? (d.fontSize as number | undefined) : undefined,
        font_bold: nt === 'text' ? (d.bold as boolean | undefined) : undefined,
        font_italic: nt === 'text' ? (d.italic as boolean | undefined) : undefined,
        font_family: nt === 'text' ? (d.fontFamily as string | undefined) : undefined,
        text_align: nt === 'text' ? (d.textAlign as 'left' | 'center' | 'right' | undefined) : undefined,
        width: nt === 'shape' && Number.isFinite(d.width) ? d.width : undefined,
        height: nt === 'shape' && Number.isFinite(d.height) ? d.height : undefined,
        shape_kind: nt === 'shape' ? (d.shapeKind as 'rect' | 'ellipse' | 'diamond' | undefined) : undefined,
        fill_color: nt === 'shape' ? (d.fillColor as string | undefined) : undefined,
        stroke_width: nt === 'shape' && Number.isFinite(d.strokeWidth) ? d.strokeWidth : undefined,
        corner_radius: nt === 'shape' && Number.isFinite(d.cornerRadius) ? d.cornerRadius : undefined,
        rotation: Number.isFinite(d.rotation) ? d.rotation : undefined,
        opacity: Number.isFinite(d.opacity) ? d.opacity : undefined,
      }
    }
    return {
      node_id: n.id,
      paper_id: d.paperId as string,
      x: n.position.x,
      y: n.position.y,
      color: d.color as string | undefined,
      hover_source: d.hoverSource as string | undefined,
    }
  })
}

function extractCanvasEdges(): CEdge[] {
  return (edges.value as VfEdge[]).map((e): CEdge => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const d = e.data as any
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const style = e.style as any
    const strokeWidth = Number(d?.edgeStrokeWidth ?? style?.strokeWidth ?? 1.8)
    const controlPoints = Array.isArray(d?.controlPoints)
      ? d.controlPoints.filter((point: unknown) => {
          if (!point || typeof point !== 'object') return false
          const maybe = point as { x?: unknown; y?: unknown }
          return typeof maybe.x === 'number' && Number.isFinite(maybe.x) &&
            typeof maybe.y === 'number' && Number.isFinite(maybe.y)
        })
      : []
    return {
      edge_id: e.id,
      from_node_id: e.source,
      to_node_id: e.target,
      source_handle: e.sourceHandle ?? undefined,
      target_handle: e.targetHandle ?? undefined,
      label: typeof e.label === 'string' ? e.label || undefined : undefined,
      color: d?.edgeColor ?? style?.stroke ?? undefined,
      stroke_width: Math.abs(strokeWidth - 1.8) > 0.01 ? strokeWidth : undefined,
      control_points: controlPoints.length ? controlPoints : undefined,
    }
  })
}

// ── Load canvas ───────────────────────────────────────────────────────────────

async function loadCanvas(id: string) {
  await canvasStore.openCanvas(id)
  const cv = canvasStore.currentCanvas
  if (!cv) return

  noteTitlesMap.value = new Map()
  nodes.value = buildVfNodes(cv.nodes)
  edges.value = buildVfEdges(cv.edges)

  loadNoteTitles(cv.nodes)

  await nextTick()
  if (cv.viewport.zoom) {
    setViewport({
      x: cv.viewport.offset_x,
      y: cv.viewport.offset_y,
      zoom: cv.viewport.zoom,
    })
  } else {
    fitView({ padding: 0.2 })
  }
  // Seed the history baseline for this canvas. Loading is NOT an undoable step.
  history.reset()
}

// ── Persist canvas (debounced) ────────────────────────────────────────────────

function triggerSave() {
  const cv = canvasStore.currentCanvas
  if (!cv) return
  const vp = getViewport()
  canvasStore.currentCanvas = {
    ...cv,
    nodes: extractCanvasNodes(),
    edges: extractCanvasEdges(),
    viewport: {
      offset_x: vp.x,
      offset_y: vp.y,
      zoom: vp.zoom,
    },
  }
  canvasStore.scheduleSave()
}

// ── Undo / redo history (snapshot based) ──────────────────────────────────────
const history = useCanvasHistory({
  capture: (): CanvasSnapshot => ({
    nodes: extractCanvasNodes(),
    edges: extractCanvasEdges(),
  }),
  restore: (snap) => {
    nodes.value = buildVfNodes(snap.nodes)
    edges.value = buildVfEdges(snap.edges)
  },
  persist: triggerSave,
})

// Record a structural change into history. Called from every real mutation
// (add/remove/drag-stop/color/size/paste/accept/etc). No-op while restoring.
function recordHistory() {
  history.commit()
}

// ── Vue Flow event handlers ───────────────────────────────────────────────────

onConnectStart(({ nodeId, handleId }) => {
  connectStart.value = { nodeId, handleId }
})

onConnectEnd(() => {
  connectStart.value = null
})

onConnect((params: Connection) => {
  const directed = normalizeDirectedConnection(params)
  connectStart.value = null
  if (!directed.source || !directed.target) return
  // Prevent duplicate edges
  const exists = edges.value.some(
    e => e.source === directed.source && e.target === directed.target
  )
  if (exists) return

  const newEdge: VfEdge = {
    id: newEdgeId(),
    source: directed.source,
    target: directed.target,
    sourceHandle: directed.sourceHandle,
    targetHandle: directed.targetHandle,
    markerEnd: MarkerType.ArrowClosed,
    style: { strokeWidth: 1.8 },
    type: 'adjustable',
    labelStyle: { fontSize: '11px' },
    labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
    data: { edgeStrokeWidth: 1.8 },
  }
  addEdges([newEdge])
  triggerSave()
  recordHistory()
})

const isDraggingNode = ref(false)

onNodeDrag(() => {
  isDraggingNode.value = true
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
  hoverNodeId.value = null
  hoverContent.value = ''
  hoverLoading.value = false
})

onNodeDragStop(() => {
  isDraggingNode.value = false
  triggerSave()
  recordHistory()
})

onNodesChange(() => {
  // Fires on selection, position changes, etc.
})

onEdgesChange(() => {
  // Fires on edge changes
})

// ── Canvas sidebar actions ────────────────────────────────────────────────────

async function handleCreateCanvas() {
  const name = newCanvasName.value.trim() || t('canvas.newCanvasName')
  creatingCanvas.value = true
  try {
    const cv = await canvasStore.createCanvas(name)
    newCanvasName.value = ''
    await loadCanvas(cv.id)
  } finally {
    creatingCanvas.value = false
  }
}

async function handleSelectCanvas(id: string) {
  if (canvasStore.currentCanvas?.id === id) return
  // Save current before switching
  if (canvasStore.currentCanvas) {
    triggerSave()
    await canvasStore.persistCanvas()
  }
  await loadCanvas(id)
}

function startRename(id: string, currentName: string) {
  renamingId.value = id
  renamingValue.value = currentName
  nextTick(() => {
    const el = document.querySelector('.rename-input') as HTMLInputElement | null
    el?.focus()
    el?.select()
  })
}

async function commitRename(id: string) {
  const name = renamingValue.value.trim()
  if (name) {
    await canvasStore.renameCanvas(id, name)
  }
  renamingId.value = null
}

function confirmDelete(id: string) {
  deleteConfirmId.value = id
}

async function doDelete(id: string) {
  await canvasStore.deleteCanvas(id)
  deleteConfirmId.value = null
  // If we deleted the current canvas, load next available
  const next = canvasStore.canvasList[0]
  if (next) {
    await loadCanvas(next.id)
  }
}

// ── Add paper to canvas ───────────────────────────────────────────────────────

function openPaperPicker() {
  pickerSearch.value = ''
  pickerError.value = ''
  showPaperPicker.value = true
}

function addPaperToCanvas(paper: PaperIndexEntry) {
  recordPaperAccess(paper.slug)
  const cv = canvasStore.currentCanvas
  if (!cv) return

  // Prevent duplicates
  const exists = nodes.value.some(n => n.data.paperId === paper.id)
  if (exists) {
    pickerError.value = t('canvas.alreadyAdded')
    return
  }

  const nodeId = newNodeId()
  const vp = getViewport()
  const el = flowContainerRef.value
  const w = el ? el.clientWidth : 800
  const h = el ? el.clientHeight : 600
  const x = (-vp.x + w / 2) / vp.zoom - 100
  const y = (-vp.y + h / 2) / vp.zoom - 60

  const newNode: VfNode = {
    id: nodeId,
    type: 'paper',
    position: { x, y },
    data: {
      title: paper.title,
      authors: paper.authors,
      year: paper.year,
      venue: paper.venue,
      paperId: paper.id,
      nodeId,
      valid: true,
    },
  }

  addNodes([newNode])
  showPaperPicker.value = false
  triggerSave()
  recordHistory()
}

// ── Node hover tooltip ────────────────────────────────────────────────────────

function onNodeMouseEnter(event: NodeMouseEvent) {
  const nd = event.node
  const mouseEvt = event.event as MouseEvent
  hoverPos.value = { x: mouseEvt.clientX, y: mouseEvt.clientY }

  if (hoverTimer) clearTimeout(hoverTimer)
  hoverTimer = setTimeout(async () => {
    if (isDraggingNode.value) return
    hoverNodeId.value = nd.id

    const source = nd.data.hoverSource ?? canvasStore.settings.hover_content_source
    if (source === 'none') {
      hoverContent.value = ''
      return
    }

    hoverLoading.value = true
    hoverContent.value = ''
    try {
      const raw = await canvasStore.getNodeDisplayContent(nd.data.paperId, source)
      hoverContent.value = raw
    } finally {
      hoverLoading.value = false
    }
  }, 600)
}

function onNodeMouseLeave() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
  hoverNodeId.value = null
  hoverContent.value = ''
  hoverLoading.value = false
}

const renderedHoverContent = computed(() => {
  if (!hoverContent.value) return ''
  return renderMarkdown(hoverContent.value)
})

// ── Context menu ──────────────────────────────────────────────────────────────

function onNodeContextMenu(event: NodeMouseEvent) {
  event.event.preventDefault()
  ctxMenu.value = {
    show: true,
    x: (event.event as MouseEvent).clientX,
    y: (event.event as MouseEvent).clientY,
    nodeId: event.node.id,
    edgeId: null,
  }
}

function onEdgeContextMenu(event: EdgeMouseEvent) {
  event.event.preventDefault()
  ctxMenu.value = {
    show: true,
    x: (event.event as MouseEvent).clientX,
    y: (event.event as MouseEvent).clientY,
    nodeId: null,
    edgeId: event.edge.id,
  }
}

function closeCtxMenu() {
  ctxMenu.value = { show: false, x: 0, y: 0, nodeId: null, edgeId: null }
}

async function ctxOpenInMain() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  const nd = (nodes.value as VfNode[]).find(n => n.id === nodeId)
  if (!nd) return
  await tauriEmit('canvas-open-paper', { paperId: nd.data.paperId })
  closeCtxMenu()
}

function ctxRemoveNode() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  removeNodes([nodeId])
  removeEdges((edges.value as VfEdge[]).filter(e => e.source === nodeId || e.target === nodeId).map(e => e.id))
  closeCtxMenu()
  triggerSave()
  recordHistory()
}

function ctxRemoveEdge() {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  removeEdges([edgeId])
  closeCtxMenu()
  triggerSave()
  recordHistory()
}

function ctxEditEdgeLabel() {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  const edge = edges.value.find(e => e.id === edgeId)
  editingEdgeId.value = edgeId
  editingEdgeLabel.value = typeof edge?.label === 'string' ? edge.label : ''
  closeCtxMenu()
}

function commitEdgeLabel() {
  if (!editingEdgeId.value) return
  edges.value = edges.value.map(e =>
    e.id === editingEdgeId.value
      ? { ...e, label: editingEdgeLabel.value || undefined }
      : e
  )
  editingEdgeId.value = null
  triggerSave()
  recordHistory()
}

// ── Node / Edge shared color palette ─────────────────────────────────────────

const COLOR_PALETTE = [
  { label: '默认', value: undefined as string | undefined },
  { label: '蓝色', value: '#3b82f6' },
  { label: '绿色', value: '#22c55e' },
  { label: '琥珀', value: '#f59e0b' },
  { label: '红色', value: '#ef4444' },
  { label: '紫色', value: '#a855f7' },
  { label: '灰色', value: '#9ca3af' },
]

const EDGE_WIDTHS = [
  { label: '细', value: 1 },
  { label: '中', value: 1.8 },
  { label: '粗', value: 3.5 },
]

const ctxCurrentEdgeData = computed(() => {
  const id = ctxMenu.value.edgeId
  if (!id) return { color: undefined as string | undefined, strokeWidth: 1.8 }
  const edge = edges.value.find(e => e.id === id)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const d = edge?.data as any
  return { color: d?.edgeColor as string | undefined, strokeWidth: d?.edgeStrokeWidth ?? 1.8 }
})

function ctxSetEdgeColor(color: string | undefined) {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  edges.value = edges.value.map(e => {
    if (e.id !== edgeId) return e
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const style = { ...(e.style as any) }
    if (color) style.stroke = color
    else delete style.stroke
    return {
      ...e,
      style,
      markerEnd: color ? { type: MarkerType.ArrowClosed, color } : MarkerType.ArrowClosed,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      data: { ...(e.data as any), edgeColor: color },
    }
  })
  triggerSave()
  recordHistory()
}

function ctxSetEdgeStrokeWidth(w: number) {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  edges.value = edges.value.map(e => {
    if (e.id !== edgeId) return e
    return {
      ...e,
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      style: { ...(e.style as any), strokeWidth: w },
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      data: { ...(e.data as any), edgeStrokeWidth: w },
    }
  })
  triggerSave()
  recordHistory()
}

// ── Node color ────────────────────────────────────────────────────────────────

const ctxCurrentNodeColor = computed(() => {
  const id = ctxMenu.value.nodeId
  if (!id) return undefined as string | undefined
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (nodes.value.find(n => n.id === id)?.data as any)?.color as string | undefined
})

function ctxSetNodeColor(color: string | undefined) {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n => {
    if (n.id !== nodeId) return n
    return { ...n, data: { ...n.data, color } }
  })
  triggerSave()
  recordHistory()
}

// ── M10: Accept suggestion as real edge ───────────────────────────────────────
function acceptSuggestion(s: SuggestedEdge, index = 0) {
  const fromNode = (nodes.value as any[]).find(n => n.data?.paperId === s.from_paper_id)
  const toNode = (nodes.value as any[]).find(n => n.data?.paperId === s.to_paper_id)
  if (!fromNode || !toNode) return

  // Skip if an edge with the same source/target already exists.
  const exists = edges.value.some(
    e => e.source === fromNode.id && e.target === toNode.id
  )
  if (exists) return

  const newEdge = {
    id: newEdgeId(index),
    source: fromNode.id,
    target: toNode.id,
    type: 'adjustable',
    markerEnd: MarkerType.ArrowClosed,
    style: { strokeWidth: 1.8 },
    labelStyle: { fontSize: '11px' },
    labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
    label: '',
    data: { edgeStrokeWidth: 1.8 },
  }
  addEdges([newEdge])
  triggerSave()
  recordHistory()
}

function acceptAllSuggestions(suggestions: SuggestedEdge[]) {
  suggestions.forEach((s, i) => acceptSuggestion(s, i))
}

// ── M10: Auto layout (enhanced) ──────────────────────────────────────────────
async function applyLayout(layout: 'timeline' | 'topological', direction: 'horizontal' | 'vertical') {
  if (!canvasStore.currentCanvas) return
  if (!window.confirm(t('canvas.layoutConfirm'))) return

  // Save current positions for undo
  layoutHistory.value = (nodes.value as any[]).map(n => ({
    node_id: n.id,
    x: n.position.x,
    y: n.position.y,
  }))

  applyingLayout.value = true
  showLayoutMenu.value = false
  try {
    const positions = await invoke<NodePosition[]>('compute_canvas_layout', {
      canvasId: canvasStore.currentCanvas.id,
      layout,
      direction,
    })

    const posMap = new Map(positions.map(p => [p.node_id, p]))
    nodes.value = (nodes.value as any[]).map(n => {
      const pos = posMap.get(n.id)
      return pos ? { ...n, position: { x: pos.x, y: pos.y } } : n
    })
    await nextTick()
    fitView({ padding: 0.12 })
    triggerSave()
    recordHistory()
  } catch (e) {
    console.error('Layout failed:', e)
  } finally {
    applyingLayout.value = false
  }
}

function undoLayout() {
  if (!layoutHistory.value.length) return
  const posMap = new Map(layoutHistory.value.map(p => [p.node_id, p]))
  nodes.value = (nodes.value as any[]).map(n => {
    const pos = posMap.get(n.id)
    return pos ? { ...n, position: { x: pos.x, y: pos.y } } : n
  })
  layoutHistory.value = []
  triggerSave()
}

// ── Auto layout ───────────────────────────────────────────────────────────────

function autoLayout() {
  // Simple topological + chronological layout
  // 1. Build adjacency for upstream->downstream direction
  const edgeMap = new Map<string, string[]>() // nodeId → downstream nodeIds
  const inDegree = new Map<string, number>()
  nodes.value.forEach(n => { edgeMap.set(n.id, []); inDegree.set(n.id, 0) })
  edges.value.forEach(e => {
    edgeMap.get(e.source)?.push(e.target)
    inDegree.set(e.target, (inDegree.get(e.target) ?? 0) + 1)
  })

  // Topological BFS layers
  const layers: string[][] = []
  const queue: string[] = []
  inDegree.forEach((deg, id) => { if (deg === 0) queue.push(id) })

  // Sort roots by year (earliest first)
  queue.sort((a, b) => {
    const na = nodes.value.find(n => n.id === a)
    const nb = nodes.value.find(n => n.id === b)
    return (na?.data.year ?? 9999) - (nb?.data.year ?? 9999)
  })

  const visited = new Set<string>()
  while (queue.length > 0) {
    const layer = [...queue]
    layers.push(layer)
    queue.length = 0
    layer.forEach(id => {
      visited.add(id)
      edgeMap.get(id)?.forEach(child => {
        if (!visited.has(child)) {
          const newDeg = (inDegree.get(child) ?? 1) - 1
          inDegree.set(child, newDeg)
          if (newDeg === 0) queue.push(child)
        }
      })
    })
  }

  // Assign positions
  const colWidth = 280
  const rowHeight = 180
  const updatedPositions = new Map<string, { x: number; y: number }>()

  layers.forEach((layer, colIdx) => {
    layer.forEach((id, rowIdx) => {
      updatedPositions.set(id, {
        x: colIdx * colWidth + 60,
        y: rowIdx * rowHeight + 60,
      })
    })
  })

  // Handle any isolated nodes (in cycles or not reached)
  let isolatedRow = layers.length
  nodes.value.forEach(n => {
    if (!updatedPositions.has(n.id)) {
      updatedPositions.set(n.id, { x: isolatedRow * colWidth + 60, y: 60 })
      isolatedRow++
    }
  })

  nodes.value = nodes.value.map(n => ({
    ...n,
    position: updatedPositions.get(n.id) ?? n.position,
  }))

  triggerSave()
  recordHistory()
  nextTick(() => fitView({ padding: 0.1 }))
}

// ── Keyboard shortcuts ────────────────────────────────────────────────────────

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    closeCtxMenu()
    showPaperPicker.value = false
    editingEdgeId.value = null
  }
  if ((e.key === 'Delete' || e.key === 'Backspace') && ctxMenu.value.nodeId) {
    ctxRemoveNode()
  }

  // Undo / redo. Skip when the user is typing in an input / editable region so
  // the browser's native text undo keeps working there.
  const el = e.target as HTMLElement | null
  if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return
  const meta = e.metaKey || e.ctrlKey
  if (!meta) return
  const key = e.key.toLowerCase()
  if (key === 'z' && !e.shiftKey) {
    e.preventDefault()
    history.undo()
  } else if ((key === 'z' && e.shiftKey) || key === 'y') {
    e.preventDefault()
    history.redo()
  }
}

// ── Document click: close context menu ───────────────────────────────────────

function onDocClick(e: MouseEvent) {
  if (ctxMenu.value.show) {
    const menu = document.querySelector('.canvas-ctx-menu')
    if (menu && !menu.contains(e.target as Element)) closeCtxMenu()
  }
}

function onEdgeControlChanged() {
  triggerSave()
  recordHistory()
}

// Before an image export, fit all nodes into the viewport so nothing is cropped.
function onExportFit() {
  fitView({ padding: 0.12 })
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

let unlisten: UnlistenFn | null = null
let unlistenSwitch: UnlistenFn | null = null

onMounted(async () => {
  await watchWindowSize()
  await Promise.all([
    canvasStore.loadList(),
    canvasStore.loadSettings(),
  ])

  // Auto-open first canvas if available
  if (canvasStore.canvasList.length > 0) {
    await loadCanvas(canvasStore.canvasList[0].id)
  }

  // Listen for paper library changes from main window
  unlisten = await listen('library-updated', async () => {
    await library.refresh()
  }).catch(() => null)

  // Listen for canvas-switch from main window sidebar
  unlistenSwitch = await listen<{ canvasId: string }>('canvas-switch', async (e) => {
    await loadCanvas(e.payload.canvasId)
  }).catch(() => null)

  document.addEventListener('keydown', onKeydown)
  document.addEventListener('pointerdown', onDocClick)
  window.addEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.addEventListener('argus-canvas-export-fit', onExportFit)
})

onUnmounted(() => {
  unlisten?.()
  unlistenSwitch?.()
  unlistenWindowResize?.()
  document.removeEventListener('keydown', onKeydown)
  document.removeEventListener('pointerdown', onDocClick)
  window.removeEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.removeEventListener('argus-canvas-export-fit', onExportFit)
  if (hoverTimer) clearTimeout(hoverTimer)
  if (windowResizeTimer) clearTimeout(windowResizeTimer)
  saveWindowSize()
})

// Watch for library papers changes to update node validity
watch(() => library.papers, () => {
  nodes.value = nodes.value.map(n => {
    // Only paper nodes carry library-backed metadata; leave annotation nodes
    // (text / shape / line) untouched so their data isn't clobbered.
    if (n.type !== 'paper') return n
    const paper = paperById(n.data.paperId)
    return {
      ...n,
      data: {
        ...n.data,
        title: paper?.title ?? n.data.title,
        authors: paper?.authors ?? n.data.authors,
        year: paper?.year ?? n.data.year,
        valid: !!paper,
      },
    }
  })
})
</script>

<template>
  <div class="canvas-root">
    <!-- Left sidebar: canvas list -->
    <div class="canvas-sidebar">
      <div class="sidebar-header">
        <span class="sidebar-title">{{ t('canvas.title') }}</span>
      </div>

      <!-- New canvas input -->
      <div class="new-canvas-row">
        <input
          v-model="newCanvasName"
          class="new-canvas-input"
          :placeholder="t('canvas.namePlaceholder')"
          @keydown.enter="handleCreateCanvas"
          :disabled="creatingCanvas"
        />
        <button class="new-canvas-btn" @click="handleCreateCanvas" :disabled="creatingCanvas" :title="t('canvas.newCanvas')">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
          </svg>
        </button>
      </div>

      <!-- Canvas list -->
      <div v-if="canvasStore.canvasList.length === 0" class="no-canvases">
        <p>{{ t('canvas.noCanvases') }}</p>
        <p class="no-canvases-hint">{{ t('canvas.noCanvasesHint') }}</p>
      </div>

      <div class="canvas-list">
        <div
          v-for="entry in canvasStore.canvasList"
          :key="entry.id"
          class="canvas-item"
          :class="{ 'canvas-item--active': canvasStore.currentCanvas?.id === entry.id }"
          @click="handleSelectCanvas(entry.id)"
        >
          <!-- Rename mode -->
          <template v-if="renamingId === entry.id">
            <input
              v-model="renamingValue"
              class="rename-input"
              @keydown.enter="commitRename(entry.id)"
              @keydown.escape="renamingId = null"
              @blur="commitRename(entry.id)"
              @click.stop
            />
          </template>
          <template v-else>
            <div class="canvas-item-name">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="canvas-icon">
                <circle cx="8" cy="8" r="3"/><circle cx="16" cy="8" r="3"/>
                <circle cx="12" cy="16" r="3"/>
                <line x1="11" y1="8" x2="13" y2="8"/>
                <line x1="9.5" y1="10.5" x2="11" y2="13.5"/>
                <line x1="14.5" y1="10.5" x2="13" y2="13.5"/>
              </svg>
              <span>{{ entry.name }}</span>
            </div>
            <div class="canvas-item-meta">{{ entry.node_count }} 节点</div>
          </template>

          <!-- Delete confirm -->
          <div v-if="deleteConfirmId === entry.id" class="delete-confirm" @click.stop>
            <span>确定删除？</span>
            <button class="btn-danger-xs" @click="doDelete(entry.id)">删除</button>
            <button class="btn-xs" @click="deleteConfirmId = null">取消</button>
          </div>
          <div v-else class="canvas-item-actions" @click.stop>
            <button class="icon-btn" :title="t('canvas.rename')" @click="startRename(entry.id, entry.name)">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
              </svg>
            </button>
            <button class="icon-btn icon-btn--danger" :title="t('canvas.delete')" @click="confirmDelete(entry.id)">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/>
                <path d="M10 11v6"/><path d="M14 11v6"/><path d="M9 6V4h6v2"/>
              </svg>
            </button>
          </div>
        </div>
      </div>
    </div>

    <!-- Main canvas area -->
    <div class="canvas-main">
      <!-- Canvas toolbar -->
      <div class="canvas-toolbar">
        <span class="canvas-name">{{ canvasStore.currentCanvas?.name || t('canvas.noCanvases') }}</span>
        <div class="toolbar-actions">
          <!-- Suggest Edges -->
          <button
            v-if="canvasStore.currentCanvas"
            class="tb-action-btn"
            :class="{ 'tb-action-btn--active': showSuggestPanel }"
            @click="showSuggestPanel = !showSuggestPanel"
            title="建议连线"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M9.5 14.5 L14.5 9.5"/><circle cx="7" cy="17" r="2.5"/><circle cx="17" cy="7" r="2.5"/>
              <circle cx="17" cy="17" r="2.5"/>
            </svg>
            {{ t('canvas.suggestEdges') }}
          </button>

          <!-- Auto Layout dropdown -->
          <div v-if="canvasStore.currentCanvas" class="layout-wrap">
            <button
              class="tb-action-btn"
              @click="showLayoutMenu = !showLayoutMenu"
              :disabled="applyingLayout"
            >
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="2" y="3" width="5" height="5" rx="1"/>
                <rect x="17" y="3" width="5" height="5" rx="1"/>
                <rect x="9.5" y="16" width="5" height="5" rx="1"/>
                <line x1="4.5" y1="8" x2="12" y2="16"/>
                <line x1="19.5" y1="8" x2="12" y2="16"/>
              </svg>
              {{ t('canvas.autoLayout') }}
            </button>

            <div v-if="showLayoutMenu" class="layout-menu" @mouseleave="showLayoutMenu = false">
              <div class="layout-menu-section">{{ t('canvas.layoutTimeline') }}</div>
              <button class="layout-menu-item" @click="applyLayout('timeline', 'horizontal')">{{ t('canvas.layoutHorizontal') }}</button>
              <button class="layout-menu-item" @click="applyLayout('timeline', 'vertical')">{{ t('canvas.layoutVertical') }}</button>
              <div class="layout-menu-sep" />
              <div class="layout-menu-section">{{ t('canvas.layoutTopological') }}</div>
              <button class="layout-menu-item" @click="applyLayout('topological', 'horizontal')">{{ t('canvas.layoutHorizontal') }}</button>
              <button class="layout-menu-item" @click="applyLayout('topological', 'vertical')">{{ t('canvas.layoutVertical') }}</button>
              <template v-if="layoutHistory.length">
                <div class="layout-menu-sep" />
                <button class="layout-menu-item layout-menu-item--undo" @click="undoLayout">↩ {{ t('canvas.layoutUndo') }}</button>
              </template>
            </div>
          </div>

          <!-- Export -->
          <button
            v-if="canvasStore.currentCanvas"
            class="tb-action-btn"
            @click="showExportDialog = true"
            title="导出图片"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
              <polyline points="7 10 12 15 17 10"/>
              <line x1="12" y1="15" x2="12" y2="3"/>
            </svg>
            {{ t('canvas.exportImage') }}
          </button>

          <!-- Add Paper -->
          <button
            v-if="canvasStore.currentCanvas"
            class="tb-action-btn tb-action-btn--accent"
            @click="openPaperPicker"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
              <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
            {{ t('canvas.addPaper') }}
          </button>
        </div>
      </div>

      <!-- Vue Flow canvas + Suggest Panel side-by-side -->
      <div class="canvas-content">
      <div v-if="canvasStore.currentCanvas" class="flow-wrap" ref="flowContainerRef">
        <VueFlow
          v-model:nodes="nodes"
          v-model:edges="edges"
          :node-types="nodeTypes"
          :edge-types="edgeTypes"
          :connection-mode="ConnectionMode.Loose"
          :default-edge-options="{ type: 'adjustable', markerEnd: MarkerType.ArrowClosed }"
          :snap-to-grid="false"
          :pan-on-drag="PAN_ON_DRAG_BUTTONS"
          :selection-key-code="true"
          :elements-selectable="true"
          :select-nodes-on-drag="true"
          fit-view-on-init
          class="canvas-flow"
          @node-mouse-enter="onNodeMouseEnter"
          @node-mouse-leave="onNodeMouseLeave"
          @node-context-menu="onNodeContextMenu"
          @edge-context-menu="onEdgeContextMenu"
          @nodes-change="triggerSave"
          @edges-change="triggerSave"
        >
          <Background pattern-color="var(--border-subtle, #e5e7eb)" :gap="24" />
          <Controls />
          <MiniMap class="canvas-minimap" />
        </VueFlow>

        <!-- Hover tooltip -->
        <Teleport to="body">
          <div
            v-if="hoverNodeId && (hoverLoading || hoverContent)"
            class="hover-tooltip"
            :style="{ left: `${hoverPos.x}px`, top: `${hoverPos.y}px` }"
          >
            <div v-if="hoverLoading" class="hover-loading">{{ t('canvas.hoverLoading') }}</div>
            <div
              v-else-if="hoverContent"
              class="hover-content"
              v-html="renderedHoverContent"
            />
            <div v-else class="hover-empty">{{ t('canvas.noHoverContent') }}</div>
          </div>
        </Teleport>
      </div>

      <!-- No canvas selected -->
      <div v-if="!canvasStore.currentCanvas" class="no-canvas-selected">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" style="opacity:0.2">
          <circle cx="8" cy="8" r="3"/><circle cx="16" cy="8" r="3"/>
          <circle cx="12" cy="16" r="3"/>
          <line x1="11" y1="8" x2="13" y2="8"/>
          <line x1="9.5" y1="10.5" x2="11" y2="13.5"/>
          <line x1="14.5" y1="10.5" x2="13" y2="13.5"/>
        </svg>
        <p>{{ t('canvas.noCanvases') }}</p>
        <p class="hint">{{ t('canvas.noCanvasesHint') }}</p>
      </div>

      <!-- Suggest Panel (side panel, shown when showSuggestPanel is true) -->
      <SuggestPanel
        v-if="showSuggestPanel && canvasStore.currentCanvas"
        :canvas-id="canvasStore.currentCanvas.id"
        :paper-names="paperNames"
        @accept="acceptSuggestion"
        @accept-all="acceptAllSuggestions"
        @close="showSuggestPanel = false"
      />
      </div>
    </div>

    <!-- Paper picker dialog -->
    <Teleport to="body">
      <div v-if="showPaperPicker" class="modal-overlay" @click.self="showPaperPicker = false">
        <div class="picker-dialog">
          <div class="picker-header">
            <span class="picker-title">{{ t('canvas.addPaperTitle') }}</span>
            <button class="close-btn" @click="showPaperPicker = false">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>
          <input
            v-model="pickerSearch"
            class="picker-search"
            :placeholder="t('canvas.searchPapers')"
            autofocus
          />
          <p v-if="pickerError" class="picker-error">{{ pickerError }}</p>
          <div class="picker-list">
            <div v-if="filteredPapers.length === 0" class="picker-empty">{{ t('canvas.noPapers') }}</div>
            <div
              v-for="paper in filteredPapers"
              :key="paper.id"
              class="picker-item"
              @click="addPaperToCanvas(paper)"
            >
              <div class="picker-item-title">{{ paper.title }}</div>
              <div class="picker-item-meta">
                <span>{{ paper.authors.slice(0, 2).join(', ') }}{{ paper.authors.length > 2 ? ' 等' : '' }}</span>
                <span v-if="paper.year">{{ paper.year }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- Context menu -->
    <Teleport to="body">
      <div
        v-if="ctxMenu.show"
        class="canvas-ctx-menu"
        :style="{ left: `${ctxMenu.x}px`, top: `${ctxMenu.y}px` }"
      >
        <!-- Node context -->
        <template v-if="ctxMenu.nodeId">
          <!-- Node color picker -->
          <div class="ctx-style-section">
            <span class="ctx-style-label">颜色</span>
            <div class="ctx-color-row">
              <button
                v-for="c in COLOR_PALETTE"
                :key="String(c.value)"
                class="ctx-color-swatch"
                :class="{ 'ctx-color-swatch--active': ctxCurrentNodeColor === c.value }"
                :title="c.label"
                :style="c.value ? { background: c.value } : {}"
                @click="ctxSetNodeColor(c.value)"
              >
                <template v-if="!c.value">
                  <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                    <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                    <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                  </svg>
                </template>
              </button>
            </div>
          </div>
          <div class="ctx-divider" />
          <button class="ctx-item" @click="ctxOpenInMain">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
              <polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/>
            </svg>
            {{ t('canvas.openInMain') }}
          </button>
          <div class="ctx-divider" />
          <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/>
              <path d="M9 6V4h6v2"/>
            </svg>
            {{ t('canvas.removePaper') }}
          </button>
        </template>
        <!-- Edge context -->
        <template v-if="ctxMenu.edgeId">
          <!-- Color swatches -->
          <div class="ctx-style-section">
            <span class="ctx-style-label">颜色</span>
            <div class="ctx-color-row">
              <button
                v-for="c in COLOR_PALETTE"
                :key="String(c.value)"
                class="ctx-color-swatch"
                :class="{ 'ctx-color-swatch--active': ctxCurrentEdgeData.color === c.value }"
                :title="c.label"
                :style="c.value ? { background: c.value } : {}"
                @click="ctxSetEdgeColor(c.value)"
              >
                <template v-if="!c.value">
                  <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                    <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                    <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                  </svg>
                </template>
              </button>
            </div>
          </div>
          <!-- Stroke width -->
          <div class="ctx-style-section">
            <span class="ctx-style-label">粗细</span>
            <div class="ctx-width-row">
              <button
                v-for="w in EDGE_WIDTHS"
                :key="w.value"
                class="ctx-width-btn"
                :class="{ 'ctx-width-btn--active': Math.abs(ctxCurrentEdgeData.strokeWidth - w.value) < 0.1 }"
                @click="ctxSetEdgeStrokeWidth(w.value)"
              >{{ w.label }}</button>
            </div>
          </div>
          <div class="ctx-divider" />
          <button class="ctx-item" @click="ctxEditEdgeLabel">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
              <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
            </svg>
            编辑标签
          </button>
          <div class="ctx-divider" />
          <button class="ctx-item ctx-item--danger" @click="ctxRemoveEdge">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
            删除连线
          </button>
        </template>
      </div>
    </Teleport>

    <!-- Edge label editor -->
    <Teleport to="body">
      <div v-if="editingEdgeId" class="modal-overlay" @click.self="editingEdgeId = null">
        <div class="edge-label-dialog">
          <p class="edge-label-title">编辑边标签</p>
          <input
            v-model="editingEdgeLabel"
            class="edge-label-input"
            :placeholder="t('canvas.edgeLabelPlaceholder')"
            autofocus
            @keydown.enter="commitEdgeLabel"
            @keydown.escape="editingEdgeId = null"
          />
          <div class="edge-label-actions">
            <button class="btn-ghost-sm" @click="editingEdgeId = null">{{ t('canvas.edgeLabelCancel') }}</button>
            <button class="btn-accent-sm" @click="commitEdgeLabel">{{ t('canvas.edgeLabelSave') }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- Export Dialog -->
    <ExportDialog
      v-if="showExportDialog"
      :canvas-name="canvasStore.currentCanvas?.name ?? 'canvas'"
      :flow-el="flowContainerRef"
      @close="showExportDialog = false"
    />
  </div>
</template>

<style scoped>
/* ── Root layout ─────────────────────────────────────── */
.canvas-root {
  display: flex;
  width: 100vw;
  height: 100vh;
  overflow: hidden;
  background: var(--bg-primary, #fff);
  color: var(--text-primary, #111);
  font-family: var(--font-sans, system-ui, sans-serif);
}

/* ── Sidebar ─────────────────────────────────────────── */
.canvas-sidebar {
  width: 240px;
  flex-shrink: 0;
  background: var(--bg-secondary, #f9fafb);
  border-right: 1px solid var(--border-subtle, #e5e7eb);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.sidebar-header {
  padding: 16px 14px 8px;
  border-bottom: 1px solid var(--border-subtle, #e5e7eb);
}

.sidebar-title {
  font-size: 13px;
  font-weight: 700;
  color: var(--text-secondary, #6b7280);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.new-canvas-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border-subtle, #e5e7eb);
}

.new-canvas-input {
  flex: 1;
  min-width: 0;
  padding: 5px 8px;
  font-size: 12px;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 6px;
  background: var(--bg-primary, #fff);
  color: var(--text-primary, #111);
}
.new-canvas-input::placeholder { color: var(--text-tertiary, #9ca3af); }
.new-canvas-input:focus { outline: none; border-color: var(--accent, #6366f1); }

.new-canvas-btn {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  background: var(--accent, #6366f1);
  color: #fff;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  transition: opacity 0.12s;
}
.new-canvas-btn:hover { opacity: 0.85; }
.new-canvas-btn:disabled { opacity: 0.4; }

.canvas-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px 0;
}

.no-canvases {
  padding: 24px 14px;
  text-align: center;
  color: var(--text-tertiary, #9ca3af);
  font-size: 12px;
  line-height: 1.5;
}
.no-canvases-hint { font-size: 11px; margin-top: 4px; }

.canvas-item {
  position: relative;
  padding: 8px 10px;
  cursor: pointer;
  border-radius: 6px;
  margin: 0 6px 2px;
  transition: background 0.1s;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.canvas-item:hover { background: var(--bg-hover, #f3f4f6); }
.canvas-item--active { background: var(--accent-light, #e0e7ff); }

.canvas-item-name {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-primary, #111);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.canvas-icon { flex-shrink: 0; color: var(--accent, #6366f1); }

.canvas-item-meta {
  font-size: 10px;
  color: var(--text-tertiary, #9ca3af);
  padding-left: 17px;
}

.canvas-item-actions {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  display: none;
  gap: 3px;
}
.canvas-item:hover .canvas-item-actions { display: flex; }

.icon-btn {
  width: 22px;
  height: 22px;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary, #9ca3af);
  transition: background 0.1s, color 0.1s;
}
.icon-btn:hover { background: var(--bg-hover, #f3f4f6); color: var(--text-primary, #111); }
.icon-btn--danger:hover { background: #fee2e2; color: #ef4444; }

.rename-input {
  width: 100%;
  padding: 3px 6px;
  font-size: 12px;
  border: 1px solid var(--accent, #6366f1);
  border-radius: 4px;
  background: var(--bg-primary, #fff);
  color: var(--text-primary, #111);
}
.rename-input:focus { outline: none; }

.delete-confirm {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--text-secondary, #6b7280);
  padding-top: 4px;
}

.btn-xs {
  padding: 2px 8px;
  font-size: 11px;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 4px;
  background: var(--bg-primary, #fff);
  color: var(--text-secondary, #6b7280);
}
.btn-danger-xs {
  padding: 2px 8px;
  font-size: 11px;
  border-radius: 4px;
  background: #ef4444;
  color: #fff;
}

/* ── Main canvas ─────────────────────────────────────── */
.canvas-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.canvas-toolbar {
  height: 40px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  padding: 0 16px;
  gap: 12px;
  background: var(--bg-secondary, #f9fafb);
  border-bottom: 1px solid var(--border-subtle, #e5e7eb);
}

.canvas-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary, #111);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.tb-action-btn {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 500;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 6px;
  background: var(--bg-primary, #fff);
  color: var(--text-secondary, #6b7280);
  transition: background 0.12s, color 0.12s;
  white-space: nowrap;
}
.tb-action-btn:hover { background: var(--bg-hover, #f3f4f6); color: var(--text-primary, #111); }

.tb-action-btn--accent {
  background: var(--accent, #6366f1);
  color: #fff;
  border-color: var(--accent, #6366f1);
}
.tb-action-btn--accent:hover { opacity: 0.88; background: var(--accent, #6366f1); color: #fff; }

.canvas-content {
  display: flex;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.flow-wrap {
  flex: 1;
  position: relative;
  overflow: hidden;
}

.layout-wrap { position: relative; }

.layout-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  min-width: 200px;
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: var(--radius-lg, 10px);
  box-shadow: 0 4px 16px rgba(0,0,0,0.15);
  padding: 6px;
  z-index: 100;
}
.layout-menu-section {
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary, #9ca3af);
  padding: 3px 8px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.layout-menu-sep { height: 1px; background: var(--border-subtle, #e5e7eb); margin: 4px 0; }
.layout-menu-item {
  display: block;
  width: 100%;
  padding: 6px 10px;
  font-size: 12px;
  color: var(--text-primary, #111);
  text-align: left;
  border-radius: 4px;
  transition: background 0.1s;
}
.layout-menu-item:hover { background: var(--bg-hover, #f3f4f6); }
.layout-menu-item--undo { color: var(--accent, #6366f1); }

.tb-action-btn--active {
  background: var(--accent-light, #e0e7ff) !important;
  color: var(--accent, #6366f1) !important;
}

.canvas-flow {
  width: 100%;
  height: 100%;
}

.no-canvas-selected {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-secondary, #6b7280);
  font-size: 14px;
}
.no-canvas-selected .hint {
  font-size: 12px;
  color: var(--text-tertiary, #9ca3af);
}

/* ── Vue Flow overrides ───────────────────────────────── */
:deep(.vue-flow__edge-path) {
  stroke: var(--accent, #6366f1);
}
:deep(.vue-flow__controls) {
  border-radius: 8px;
  overflow: hidden;
}
:deep(.vue-flow__minimap) {
  border-radius: 8px;
  overflow: hidden;
}
.canvas-minimap {
  bottom: 12px;
  right: 12px;
}

/* ── Hover tooltip ────────────────────────────────────── */
.hover-tooltip {
  position: fixed;
  z-index: 9999;
  width: 360px;
  max-height: 400px;
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.15);
  overflow-y: auto;
  padding: 14px 16px;
  pointer-events: none;
  transform: translate(16px, -50%);
}

.hover-loading {
  color: var(--text-tertiary, #9ca3af);
  font-size: 12px;
}

.hover-empty {
  color: var(--text-tertiary, #9ca3af);
  font-size: 12px;
  font-style: italic;
}

.hover-content {
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-primary, #111);
}

:deep(.hover-content h1) { font-size: 15px; margin: 8px 0 4px; }
:deep(.hover-content h2) { font-size: 13px; margin: 6px 0 3px; }
:deep(.hover-content h3) { font-size: 12px; margin: 4px 0 2px; }
:deep(.hover-content p) { margin: 4px 0; }
:deep(.hover-content ul, .hover-content ol) { padding-left: 16px; margin: 4px 0; }
:deep(.hover-content code) {
  background: var(--bg-secondary, #f3f4f6);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 11px;
}

/* ── Paper picker dialog ──────────────────────────────── */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.35);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 9000;
}

.picker-dialog {
  width: 560px;
  max-height: 70vh;
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 12px;
  box-shadow: 0 8px 32px rgba(0,0,0,0.2);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.picker-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 16px 0;
}

.picker-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary, #111);
}

.close-btn {
  width: 24px;
  height: 24px;
  border-radius: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary, #9ca3af);
  transition: background 0.1s;
}
.close-btn:hover { background: var(--bg-hover, #f3f4f6); color: var(--text-primary, #111); }

.picker-search {
  margin: 10px 16px;
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 8px;
  background: var(--bg-secondary, #f9fafb);
  color: var(--text-primary, #111);
  width: calc(100% - 32px);
  box-sizing: border-box;
}
.picker-search:focus { outline: none; border-color: var(--accent, #6366f1); background: var(--bg-primary, #fff); }

.picker-error {
  color: #ef4444;
  font-size: 12px;
  margin: 0 16px 8px;
}

.picker-list {
  flex: 1;
  overflow-y: auto;
  padding: 0 8px 12px;
}

.picker-empty {
  text-align: center;
  color: var(--text-tertiary, #9ca3af);
  font-size: 13px;
  padding: 24px;
}

.picker-item {
  position: relative;
  padding: 9px 10px;
  border-radius: 7px;
  cursor: pointer;
  transition: background 0.1s;
}
.picker-item:hover { background: var(--bg-hover, #f3f4f6); }
.picker-item--taken { opacity: 0.5; pointer-events: none; }

.picker-item-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary, #111);
  line-height: 1.3;
  margin-bottom: 3px;
}

.picker-item-meta {
  display: flex;
  gap: 10px;
  font-size: 11px;
  color: var(--text-tertiary, #9ca3af);
}

.picker-item-badge {
  position: absolute;
  right: 10px;
  top: 50%;
  transform: translateY(-50%);
  background: var(--accent-light, #e0e7ff);
  color: var(--accent, #6366f1);
  font-size: 10px;
  font-weight: 600;
  padding: 2px 6px;
  border-radius: 4px;
}

/* ── Context menu ─────────────────────────────────────── */
.canvas-ctx-menu {
  position: fixed;
  z-index: 9500;
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0,0,0,0.15);
  padding: 4px 0;
  min-width: 170px;
}

.ctx-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 14px;
  font-size: 13px;
  color: var(--text-primary, #111);
  text-align: left;
  transition: background 0.1s;
}
.ctx-item:hover { background: var(--bg-hover, #f3f4f6); }
.ctx-item--danger { color: #ef4444; }
.ctx-item--danger:hover { background: #fee2e2; }

.ctx-divider {
  height: 1px;
  background: var(--border-subtle, #e5e7eb);
  margin: 3px 0;
}

/* Edge style controls inside context menu */
.ctx-style-section {
  padding: 5px 10px 4px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.ctx-style-label {
  font-size: 11px;
  color: var(--text-tertiary, #9ca3af);
  width: 24px;
  flex-shrink: 0;
}

.ctx-color-row {
  display: flex;
  gap: 5px;
  align-items: center;
}

.ctx-color-swatch {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  border: 1.5px solid transparent;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: transform 0.1s, border-color 0.1s;
  flex-shrink: 0;
  background: var(--bg-secondary, #f3f4f6);
}
.ctx-color-swatch:hover { transform: scale(1.2); }
.ctx-color-swatch--active { border-color: var(--text-primary, #111); }

.ctx-width-row {
  display: flex;
  gap: 4px;
}

.ctx-width-btn {
  padding: 2px 9px;
  font-size: 11px;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 4px;
  color: var(--text-secondary, #6b7280);
  background: var(--bg-primary, #fff);
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.ctx-width-btn:hover { background: var(--bg-hover, #f3f4f6); color: var(--text-primary, #111); }
.ctx-width-btn--active {
  background: var(--accent-light, #e0e7ff);
  color: var(--accent, #6366f1);
  border-color: var(--accent, #6366f1);
}

/* ── Edge label dialog ────────────────────────────────── */
.edge-label-dialog {
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.15);
  padding: 16px;
  width: 320px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.edge-label-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary, #111);
  margin: 0;
}

.edge-label-input {
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 7px;
  background: var(--bg-secondary, #f9fafb);
  color: var(--text-primary, #111);
}
.edge-label-input:focus { outline: none; border-color: var(--accent, #6366f1); }

.edge-label-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.btn-ghost-sm {
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary, #6b7280);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 6px;
  transition: background 0.1s;
}
.btn-ghost-sm:hover { background: var(--bg-hover, #f3f4f6); }

.btn-accent-sm {
  padding: 5px 14px;
  font-size: 12px;
  font-weight: 600;
  background: var(--accent, #6366f1);
  color: #fff;
  border-radius: 6px;
  transition: opacity 0.12s;
}
.btn-accent-sm:hover { opacity: 0.85; }
</style>
