<script setup lang="ts">
import {
  ref, computed, markRaw, onMounted, onUnmounted, nextTick, watch,
} from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
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
import { useCanvasStore, type DrawNodeSnapshot } from '../stores/canvas'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import { useCanvasHistory, type CanvasSnapshot } from '../composables/useCanvasHistory'
import { sortPapersByRecentAccess } from '../utils/recentPapers'
import PaperNode from './canvas/PaperNode.vue'
import AdjustableEdge from './canvas/AdjustableEdge.vue'
import TextNode from './canvas/TextNode.vue'
import ShapeNode from './canvas/ShapeNode.vue'
import LineNode from './canvas/LineNode.vue'
import ImageNode from './canvas/ImageNode.vue'
import SuggestPanel from './canvas/SuggestPanel.vue'
import ExportDialog from './canvas/ExportDialog.vue'
import type { PaperIndexEntry, CanvasNode as CNode, CanvasEdge as CEdge, SuggestedEdge, NodePosition } from '../types'

const { t } = useI18n()
const canvasStore = useCanvasStore()
const library = useLibraryStore()
const reader = useReaderStore()
const selectionStore = useSelectionStore()

const emit = defineEmits<{
  'select-paper': [slug: string]
  'close': []
}>()

// ── Vue Flow setup ────────────────────────────────────────────────────────────

const nodeTypes = markRaw({ paper: PaperNode, text: TextNode, shape: ShapeNode, line: LineNode, image: ImageNode })
const edgeTypes = markRaw({ adjustable: AdjustableEdge })

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
  onNodeDragStop,
  onNodeDrag,
  onNodeDragStart,
  getViewport,
  setViewport,
  fitView,
  screenToFlowCoordinate,
  updateNode,
  updateNodeData,
  findNode,
  getNodes,
  getSelectedNodes,
  addSelectedNodes,
  removeSelectedNodes,
} = useVueFlow()

// ── Bottom toolbar tool state ─────────────────────────────────────────────────
type CanvasTool = 'pointer' | 'text' | 'shape' | 'line'
type PointerDragMode = 'select' | 'pan'
type ShapeKind = 'rect' | 'ellipse' | 'diamond'
type LineKind = 'line' | 'arrow'
const activeTool = ref<CanvasTool>('pointer')
const pointerDragMode = ref<PointerDragMode>('select')
const pendingShapeKind = ref<ShapeKind>('rect')
const pendingLineKind = ref<LineKind>('arrow')
const DEFAULT_SHAPE_WIDTH = 160
const DEFAULT_SHAPE_HEIGHT = 100
const MIN_SHAPE_SIZE = 12
const PASTED_IMAGE_MAX_WIDTH = 420
const PASTED_IMAGE_MAX_HEIGHT = 320
const isDraftTool = computed(() => activeTool.value === 'shape' || activeTool.value === 'line')

type ShapeDraft = {
  startClientX: number
  startClientY: number
  currentClientX: number
  currentClientY: number
}
const shapeDraft = ref<ShapeDraft | null>(null)
const flowContainerRef = ref<HTMLElement | null>(null)
const lastCanvasPointer = ref<{ x: number; y: number } | null>(null)
const isPointerPanMode = computed(() => activeTool.value === 'pointer' && pointerDragMode.value === 'pan')
const pointerPanOnDrag = computed(() => {
  if (activeTool.value !== 'pointer') return false
  return pointerDragMode.value === 'pan' ? true : [1, 2]
})
const pointerSelectionKeyCode = computed(() => {
  if (activeTool.value !== 'pointer') return null
  return pointerDragMode.value === 'select' ? true : null
})
const pointerToolTitle = computed(() =>
  pointerDragMode.value === 'select'
    ? '框选：左键拖拽框选节点，点击切换为拖动画布'
    : '拖动画布：左键拖动画布，点击切换为框选'
)

function selectTool(tool: CanvasTool) {
  activeTool.value = activeTool.value === tool ? 'pointer' : tool
  if (!isDraftTool.value) resetShapeDraft()
}

function selectShape(kind: ShapeKind) {
  if (activeTool.value === 'shape' && pendingShapeKind.value === kind) {
    activeTool.value = 'pointer'
    resetShapeDraft()
    return
  }
  pendingShapeKind.value = kind
  activeTool.value = 'shape'
}

function selectLine(kind: LineKind) {
  if (activeTool.value === 'line' && pendingLineKind.value === kind) {
    activeTool.value = 'pointer'
    resetShapeDraft()
    return
  }
  pendingLineKind.value = kind
  activeTool.value = 'line'
}

function selectPointerTool() {
  if (activeTool.value === 'pointer') {
    pointerDragMode.value = pointerDragMode.value === 'select' ? 'pan' : 'select'
  } else {
    activeTool.value = 'pointer'
    resetShapeDraft()
  }
}

const shapePreviewStyle = computed(() => {
  const draft = shapeDraft.value
  const container = flowContainerRef.value
  if (!draft || !container) return {}
  const rect = container.getBoundingClientRect()
  const left = Math.min(draft.startClientX, draft.currentClientX) - rect.left
  const top = Math.min(draft.startClientY, draft.currentClientY) - rect.top
  const width = Math.abs(draft.currentClientX - draft.startClientX)
  const height = Math.abs(draft.currentClientY - draft.startClientY)
  return {
    left: `${left}px`,
    top: `${top}px`,
    width: `${width}px`,
    height: `${height}px`,
  }
})

// ── Text/Shape node inline editor ────────────────────────────────────────────
const editingAnnotationId = ref<string | null>(null)
const editingAnnotationContent = ref('')

function openAnnotationEditor(nodeId: string) {
  const node = nodes.value.find((n: any) => n.id === nodeId)
  editingAnnotationId.value = nodeId
  editingAnnotationContent.value = (node?.data as any)?.content ?? ''
}

function commitAnnotationEdit() {
  if (!editingAnnotationId.value) return
  nodes.value = nodes.value.map((n: any) =>
    n.id === editingAnnotationId.value
      ? { ...n, data: { ...n.data, content: editingAnnotationContent.value } }
      : n
  )
  editingAnnotationId.value = null
  triggerSave()
  recordHistory()
}

// ── Pane interactions → place text / draw shape node ─────────────────────────
function onPaneClick(event: MouseEvent) {
  // Clicking empty canvas deselects (so the properties panel clears).
  if (activeTool.value === 'pointer') {
    canvasStore.setSelectedNode(null)
    return
  }
  if (activeTool.value !== 'text') return
  const flowPos = screenToFlowCoordinate({ x: event.clientX, y: event.clientY })
  const nodeId = `node-${Date.now()}`
  const newNode: VfNode = {
    id: nodeId,
    type: 'text',
    position: { x: flowPos.x - 60, y: flowPos.y - 14 },
    data: { content: '文字', nodeId },
  }
  addNodes([newNode])
  triggerSave()
  recordHistory()
  openAnnotationEditor(nodeId)
  activeTool.value = 'pointer'
}

function canStartShapeDraft(event: PointerEvent) {
  const target = event.target as Element | null
  if (!target) return false
  return !target.closest([
    '.vue-flow__node',
    '.vue-flow__edge',
    '.vue-flow__controls',
    '.vue-flow__minimap',
    '.canvas-bottom-toolbar',
    '.canvas-ctx-menu',
  ].join(', '))
}

function addShapeFromDraft(draft: ShapeDraft) {
  const start = screenToFlowCoordinate({ x: draft.startClientX, y: draft.startClientY })
  const end = screenToFlowCoordinate({ x: draft.currentClientX, y: draft.currentClientY })
  let x = Math.min(start.x, end.x)
  let y = Math.min(start.y, end.y)
  let width = Math.abs(end.x - start.x)
  let height = Math.abs(end.y - start.y)

  if (width < MIN_SHAPE_SIZE || height < MIN_SHAPE_SIZE) {
    width = DEFAULT_SHAPE_WIDTH
    height = DEFAULT_SHAPE_HEIGHT
    x = start.x - width / 2
    y = start.y - height / 2
  }

  const nodeId = `node-${Date.now()}`
  const newNode: VfNode = {
    id: nodeId,
    type: 'shape',
    position: { x, y },
    data: {
      content: '',
      nodeId,
      width,
      height,
      shapeKind: pendingShapeKind.value,
    },
  }
  addNodes([newNode])
  triggerSave()
  recordHistory()
}

// Create a standalone line / arrow from a drag, storing its two endpoints as
// offsets inside the node's bounding box.
function addLineFromDraft(draft: ShapeDraft) {
  const start = screenToFlowCoordinate({ x: draft.startClientX, y: draft.startClientY })
  const end = screenToFlowCoordinate({ x: draft.currentClientX, y: draft.currentClientY })
  // Ignore an accidental click with no drag.
  if (Math.hypot(end.x - start.x, end.y - start.y) < MIN_SHAPE_SIZE) return

  const minX = Math.min(start.x, end.x)
  const minY = Math.min(start.y, end.y)
  const width = Math.max(1, Math.abs(end.x - start.x))
  const height = Math.max(1, Math.abs(end.y - start.y))

  const nodeId = `node-${Date.now()}`
  const newNode: VfNode = {
    id: nodeId,
    type: 'line',
    position: { x: minX, y: minY },
    data: {
      nodeId,
      lineKind: pendingLineKind.value,
      color: '#1a1a1a',
      strokeWidth: 2,
      width,
      height,
      x1: start.x - minX,
      y1: start.y - minY,
      x2: end.x - minX,
      y2: end.y - minY,
    },
  }
  addNodes([newNode])
  triggerSave()
  recordHistory()
}

function cleanupShapeDraftListeners() {
  window.removeEventListener('pointermove', onShapeDraftPointerMove)
  window.removeEventListener('pointerup', onShapeDraftPointerUp)
}

function resetShapeDraft() {
  shapeDraft.value = null
  cleanupShapeDraftListeners()
}

function onShapeDraftPointerMove(event: PointerEvent) {
  if (!shapeDraft.value) return
  event.preventDefault()
  shapeDraft.value = {
    ...shapeDraft.value,
    currentClientX: event.clientX,
    currentClientY: event.clientY,
  }
}

function onShapeDraftPointerUp(event: PointerEvent) {
  if (!shapeDraft.value) return
  event.preventDefault()
  const draft = {
    ...shapeDraft.value,
    currentClientX: event.clientX,
    currentClientY: event.clientY,
  }
  const wasLine = activeTool.value === 'line'
  resetShapeDraft()
  if (wasLine) addLineFromDraft(draft)
  else addShapeFromDraft(draft)
  activeTool.value = 'pointer'
}

function onFlowPointerDown(event: PointerEvent) {
  if (!isDraftTool.value || event.button !== 0 || !canStartShapeDraft(event)) return
  event.preventDefault()
  event.stopPropagation()
  shapeDraft.value = {
    startClientX: event.clientX,
    startClientY: event.clientY,
    currentClientX: event.clientX,
    currentClientY: event.clientY,
  }
  window.addEventListener('pointermove', onShapeDraftPointerMove)
  window.addEventListener('pointerup', onShapeDraftPointerUp)
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

// ── Paper picker dialog ───────────────────────────────────────────────────────

const showPaperPicker = ref(false)
const pickerSearch = ref('')
const pickerError = ref('')

const filteredPapers = computed(() => {
  const q = pickerSearch.value.trim().toLowerCase()
  const papers = sortPapersByRecentAccess(library.papers)
  if (!q) return papers
  return papers.filter(p =>
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
  paperId: string | null; nodeType: string | null
}>({ show: false, x: 0, y: 0, nodeId: null, edgeId: null, paperId: null, nodeType: null })

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

function paperBySlug(slug: string): PaperIndexEntry | undefined {
  return library.papers.find(p => p.slug === slug)
}

const noteTitlesMap = ref<Map<string, string[]>>(new Map())
let noteTitlesLoadSeq = 0

function isPaperCanvasNode(cn: CNode) {
  return (cn.node_type ?? 'paper') === 'paper' && !!cn.paper_id
}

function patchNodeNoteTitles(paperId: string, titles: string[]) {
  nodes.value = nodes.value.map(n => {
    if (n.type !== 'paper' || n.data?.paperId !== paperId) return n
    return {
      ...n,
      data: {
        ...n.data,
        noteTitles: titles,
      },
    }
  })
}

async function refreshPaperNoteTitles(slug: string, paperId?: string) {
  const paper = paperId ? paperById(paperId) : paperBySlug(slug)
  if (!paper) return
  const titles = await invoke<string[]>('get_canvas_note_titles', { slug: paper.slug })
  const next = new Map(noteTitlesMap.value)
  if (titles.length > 0) next.set(paper.id, titles)
  else next.delete(paper.id)
  noteTitlesMap.value = next
  patchNodeNoteTitles(paper.id, titles)
}

async function loadNoteTitles(cnodes: CNode[]) {
  const seq = ++noteTitlesLoadSeq
  const paperNodes = cnodes.filter(isPaperCanvasNode)
  const papers = Array.from(new Map(
    paperNodes
      .map(cn => paperById(cn.paper_id))
      .filter((paper): paper is PaperIndexEntry => !!paper)
      .map(paper => [paper.id, paper])
  ).values())

  const results = await Promise.allSettled(
    papers.map(async paper => ({
      paperId: paper.id,
      titles: await invoke<string[]>('get_canvas_note_titles', { slug: paper.slug }),
    }))
  )
  if (seq !== noteTitlesLoadSeq) return

  const next = new Map<string, string[]>()
  for (const result of results) {
    if (result.status !== 'fulfilled' || result.value.titles.length === 0) continue
    next.set(result.value.paperId, result.value.titles)
  }
  noteTitlesMap.value = next
  nodes.value = nodes.value.map(n => {
    if (n.type !== 'paper') return n
    return {
      ...n,
      data: {
        ...n.data,
        noteTitles: noteTitlesMap.value.get(n.data.paperId as string) ?? [],
      },
    }
  })
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
    if (nt === 'image') {
      return {
        id: cn.node_id,
        type: 'image',
        position: { x: cn.x, y: cn.y },
        zIndex: cn.z_index,
        style: {
          width: `${cn.width ?? PASTED_IMAGE_MAX_WIDTH}px`,
          height: `${cn.height ?? PASTED_IMAGE_MAX_HEIGHT}px`,
        },
        data: {
          nodeId: cn.node_id,
          src: cn.image_src ?? cn.content ?? '',
          alt: cn.image_alt,
          width: cn.width ?? PASTED_IMAGE_MAX_WIDTH,
          height: cn.height ?? PASTED_IMAGE_MAX_HEIGHT,
          cornerRadius: cn.corner_radius,
          rotation: cn.rotation,
          opacity: cn.opacity,
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
    const strokeWidth = ce.stroke_width ?? 1.8
    const style: { strokeWidth: number; stroke?: string } = { strokeWidth }
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
        edgeStrokeWidth: strokeWidth,
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
    if (nt === 'image') {
      return {
        node_id: n.id,
        paper_id: '',
        x: n.position.x,
        y: n.position.y,
        z_index: Number.isFinite(n.zIndex) ? (n.zIndex as number) : undefined,
        hover_source: undefined,
        node_type: 'image',
        width: Number.isFinite(d.width) ? d.width : undefined,
        height: Number.isFinite(d.height) ? d.height : undefined,
        corner_radius: Number.isFinite(d.cornerRadius) ? d.cornerRadius : undefined,
        rotation: Number.isFinite(d.rotation) ? d.rotation : undefined,
        opacity: Number.isFinite(d.opacity) ? d.opacity : undefined,
        image_src: d.src as string | undefined,
        image_alt: d.alt as string | undefined,
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
    const data = e.data as any
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const style = e.style as any
    const strokeWidth = Number(data?.edgeStrokeWidth ?? style?.strokeWidth ?? 1.8)
    const controlPoints = Array.isArray(data?.controlPoints)
      ? data.controlPoints.filter((point: unknown) => {
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
      color: data?.edgeColor ?? style?.stroke ?? undefined,
      stroke_width: Math.abs(strokeWidth - 1.8) > 0.01 ? strokeWidth : undefined,
      control_points: controlPoints.length ? controlPoints : undefined,
    }
  })
}

// ── Render current canvas from store ─────────────────────────────────────────

async function renderCanvas() {
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

// Watch for canvas switches (LeftSidebar calls canvasStore.openCanvas)
watch(
  () => canvasStore.currentCanvas?.id,
  async (id) => {
    canvasStore.setSelectedNode(null)
    if (id) await renderCanvas()
    else { nodes.value = []; edges.value = [] }
  },
  { immediate: true }
)

// ── Persist canvas (debounced) ────────────────────────────────────────────────

function triggerSave() {
  const cv = canvasStore.currentCanvas
  if (!cv) return
  const vp = getViewport()
  canvasStore.currentCanvas = {
    ...cv,
    nodes: extractCanvasNodes(),
    edges: extractCanvasEdges(),
    viewport: { offset_x: vp.x, offset_y: vp.y, zoom: vp.zoom },
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
// (add/remove/drag-stop/color/size/paste/text edit/etc). No-op while restoring.
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

// ── Smart alignment guides / snapping ─────────────────────────────────────────
// Guide lines are stored in container pixels (flow coord * zoom + viewport offset).
const snapGuides = ref<{ vertical: boolean; pos: number }[]>([])
const SNAP_PX = 6 // screen-space snap threshold
let isDraggingNode = false

onNodeDragStart(() => {
  isDraggingNode = true
  snapGuides.value = []
  clearHoverTooltip() // never show the hover preview card while dragging
})

// eslint-disable-next-line @typescript-eslint/no-explicit-any
onNodeDrag(({ node }: { node: any }) => {
  // Only snap a single dragged node (multi-drag would desync the group).
  if (canvasStore.selectedNodeIds.length > 1) { snapGuides.value = []; return }
  const g = findNode(node.id)
  if (!g) return
  const w = g.dimensions?.width ?? 0
  const h = g.dimensions?.height ?? 0
  const x = node.position.x
  const y = node.position.y
  const vp = getViewport()
  const th = SNAP_PX / Math.max(vp.zoom, 1e-4) // threshold in flow units

  const others = getNodes.value.filter(n => n.id !== node.id)
  const dragXs = [x, x + w / 2, x + w]
  const dragYs = [y, y + h / 2, y + h]

  let bestDX = 0, bestAbsX = th, lineX: number | null = null
  let bestDY = 0, bestAbsY = th, lineY: number | null = null
  for (const o of others) {
    const ow = o.dimensions?.width ?? 0
    const oh = o.dimensions?.height ?? 0
    for (const t of [o.position.x, o.position.x + ow / 2, o.position.x + ow]) {
      for (const d of dragXs) {
        const diff = t - d
        if (Math.abs(diff) < bestAbsX) { bestAbsX = Math.abs(diff); bestDX = diff; lineX = t }
      }
    }
    for (const t of [o.position.y, o.position.y + oh / 2, o.position.y + oh]) {
      for (const d of dragYs) {
        const diff = t - d
        if (Math.abs(diff) < bestAbsY) { bestAbsY = Math.abs(diff); bestDY = diff; lineY = t }
      }
    }
  }

  if (lineX !== null || lineY !== null) {
    updateNode(node.id, {
      position: { x: lineX !== null ? x + bestDX : x, y: lineY !== null ? y + bestDY : y },
    })
  }
  const guides: { vertical: boolean; pos: number }[] = []
  if (lineX !== null) guides.push({ vertical: true, pos: lineX * vp.zoom + vp.x })
  if (lineY !== null) guides.push({ vertical: false, pos: lineY * vp.zoom + vp.y })
  snapGuides.value = guides
})

onNodeDragStop(() => {
  isDraggingNode = false
  snapGuides.value = []
  triggerSave()
  recordHistory()
  // Reflect the new position in the properties panel.
  const sel = canvasStore.selectedNode
  if (sel) publishSelection(sel.nodeId)
})

// ── Resolve paper → emit + select in store ────────────────────────────────────

function selectPaperById(paperId: string) {
  const paper = library.papers.find(p => p.id === paperId)
  if (paper?.slug) {
    selectionStore.selectPaper(paper.slug)
    emit('select-paper', paper.slug)
  }
}

function openPaperById(paperId: string) {
  const paper = library.papers.find(p => p.id === paperId)
  if (!paper?.slug) return
  selectionStore.selectPaper(paper.slug)
  reader.openPaper(paper.slug, paper.title)
  canvasStore.isShown = false
  emit('select-paper', paper.slug)
}

// ── Paper node click → select in right sidebar ────────────────────────────────

function onNodeClick(event: NodeMouseEvent) {
  // Selection itself is published via onSelectionChange; here we only do the
  // paper-node side effect (select the paper in the library).
  if (event.node.type !== 'paper') return
  const paperId = event.node.data?.paperId as string | undefined
  if (paperId) selectPaperById(paperId)
}

// Keep the properties panel & multi-select state in sync with Vue Flow.
watch(getSelectedNodes, (sel) => {
  const ids = sel.map(n => n.id)
  canvasStore.setSelectedNodeIds(ids)
  canvasStore.setSelectedNode(ids.length === 1 ? snapshotFromVf(sel[0]) : null)
})

// ── Selection ↔ properties panel (DrawTab) sync ───────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function snapshotFromVf(n: any): DrawNodeSnapshot {
  const d = n.data ?? {}
  const type = (n.type ?? 'paper') as 'paper' | 'text' | 'shape' | 'line' | 'image'
  return {
    nodeId: n.id,
    type,
    lineKind: d.lineKind,
    x: Math.round(n.position.x),
    y: Math.round(n.position.y),
    width: Number.isFinite(d.width) ? Math.round(d.width) : undefined,
    height: Number.isFinite(d.height) ? Math.round(d.height) : undefined,
    rotation: d.rotation,
    opacity: d.opacity,
    cornerRadius: d.cornerRadius,
    color: d.color,
    fillColor: d.fillColor,
    strokeWidth: d.strokeWidth,
    shapeKind: d.shapeKind,
    content: d.content,
    fontFamily: d.fontFamily,
    fontSize: d.fontSize,
    bold: d.bold,
    italic: d.italic,
    textAlign: d.textAlign,
    imageSrc: d.src,
    imageAlt: d.alt,
  }
}

function publishSelection(nodeId: string) {
  const n = findNode(nodeId)
  canvasStore.setSelectedNode(n ? snapshotFromVf(n) : null)
}

// Apply a property change coming from the DrawTab panel onto the live node.
function applyNodePatch(nodeId: string, patch: Partial<DrawNodeSnapshot>) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const dataPatch: Record<string, any> = {}
  let posX: number | undefined
  let posY: number | undefined
  let newW: number | undefined
  let newH: number | undefined
  for (const [k, v] of Object.entries(patch)) {
    if (v === undefined) continue
    if (k === 'x') posX = v as number
    else if (k === 'y') posY = v as number
    else if (k === 'nodeId' || k === 'type') continue
    else {
      if (k === 'width') newW = v as number
      else if (k === 'height') newH = v as number
      if (k === 'imageSrc') dataPatch.src = v
      else if (k === 'imageAlt') dataPatch.alt = v
      else dataPatch[k] = v
    }
  }

  const n = findNode(nodeId)

  // When resizing a line, scale its two endpoints so the drawn segment keeps
  // filling the (new) bounding box instead of staying anchored to old offsets.
  if (n?.type === 'line' && (newW !== undefined || newH !== undefined)) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const d = n.data as any
    const oldW = Number.isFinite(d.width) && d.width > 0 ? (d.width as number) : 1
    const oldH = Number.isFinite(d.height) && d.height > 0 ? (d.height as number) : 1
    const sx = newW !== undefined ? newW / oldW : 1
    const sy = newH !== undefined ? newH / oldH : 1
    dataPatch.x1 = (Number.isFinite(d.x1) ? d.x1 : 0) * sx
    dataPatch.y1 = (Number.isFinite(d.y1) ? d.y1 : 0) * sy
    dataPatch.x2 = (Number.isFinite(d.x2) ? d.x2 : oldW) * sx
    dataPatch.y2 = (Number.isFinite(d.y2) ? d.y2 : oldH) * sy
  }

  if (Object.keys(dataPatch).length) updateNodeData(nodeId, dataPatch)

  // Keep Vue Flow's node dimensions in sync so alignment / box-select / snap
  // guides operate on the new size (the visual size is driven by data.width/
  // height, but dimensions are otherwise only re-measured lazily).
  if (newW !== undefined || newH !== undefined) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const style: Record<string, any> = { ...((n?.style as any) ?? {}) }
    if (newW !== undefined) style.width = `${newW}px`
    if (newH !== undefined) style.height = `${newH}px`
    updateNode(nodeId, { style })
  }

  if (posX !== undefined || posY !== undefined) {
    if (n) {
      updateNode(nodeId, {
        position: { x: posX ?? n.position.x, y: posY ?? n.position.y },
      })
    }
  }
  triggerSave()
  recordHistory()
}

watch(
  () => canvasStore.pendingPatch,
  (p) => {
    if (!p) return
    applyNodePatch(p.nodeId, p.patch)
  }
)

// ── Batch operations (multi-select) ───────────────────────────────────────────

interface NodeBox { id: string; x: number; y: number; w: number; h: number }

function selectedBoxes(): NodeBox[] {
  return canvasStore.selectedNodeIds
    .map(id => findNode(id))
    .filter((n): n is NonNullable<typeof n> => !!n)
    .map(n => ({
      id: n.id,
      x: n.position.x,
      y: n.position.y,
      w: n.dimensions?.width ?? 0,
      h: n.dimensions?.height ?? 0,
    }))
}

function refreshSelectionSnapshot() {
  const ids = canvasStore.selectedNodeIds
  if (ids.length === 1) {
    const n = findNode(ids[0])
    if (n) canvasStore.setSelectedNode(snapshotFromVf(n))
  }
}

type AlignDir = 'left' | 'hcenter' | 'right' | 'top' | 'vcenter' | 'bottom'
function alignNodes(dir: AlignDir) {
  const boxes = selectedBoxes()
  if (boxes.length < 2) return
  const minX = Math.min(...boxes.map(b => b.x))
  const maxX = Math.max(...boxes.map(b => b.x + b.w))
  const minY = Math.min(...boxes.map(b => b.y))
  const maxY = Math.max(...boxes.map(b => b.y + b.h))
  const cx = (minX + maxX) / 2
  const cy = (minY + maxY) / 2
  for (const b of boxes) {
    let x = b.x, y = b.y
    if (dir === 'left') x = minX
    else if (dir === 'right') x = maxX - b.w
    else if (dir === 'hcenter') x = cx - b.w / 2
    else if (dir === 'top') y = minY
    else if (dir === 'bottom') y = maxY - b.h
    else if (dir === 'vcenter') y = cy - b.h / 2
    updateNode(b.id, { position: { x, y } })
  }
  triggerSave()
  recordHistory()
  refreshSelectionSnapshot()
}

function distributeNodes(axis: 'h' | 'v') {
  const boxes = selectedBoxes()
  if (boxes.length < 3) return
  if (axis === 'h') {
    boxes.sort((a, b) => a.x - b.x)
    const left = boxes[0].x
    const step = (boxes[boxes.length - 1].x - left) / (boxes.length - 1)
    boxes.forEach((b, i) => updateNode(b.id, { position: { x: left + step * i, y: b.y } }))
  } else {
    boxes.sort((a, b) => a.y - b.y)
    const top = boxes[0].y
    const step = (boxes[boxes.length - 1].y - top) / (boxes.length - 1)
    boxes.forEach((b, i) => updateNode(b.id, { position: { x: b.x, y: top + step * i } }))
  }
  triggerSave()
  recordHistory()
  refreshSelectionSnapshot()
}

function setZOrder(mode: 'front' | 'back') {
  const ids = canvasStore.selectedNodeIds
  if (!ids.length) return
  const zs = getNodes.value.map(n => n.zIndex ?? 0)
  // Push above the current top (or below the current bottom). Guard the empty
  // case so we don't call Math.max/min with no args (which yields ±Infinity).
  const target = zs.length
    ? (mode === 'front' ? Math.max(...zs) + 1 : Math.min(...zs) - 1)
    : (mode === 'front' ? 1 : -1)
  ids.forEach(id => updateNode(id, { zIndex: target }))
  triggerSave()
  recordHistory()
}

function batchPatch(patch: Partial<DrawNodeSnapshot>) {
  const ids = canvasStore.selectedNodeIds
  if (!ids.length) return
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const dataPatch: Record<string, any> = {}
  for (const [k, v] of Object.entries(patch)) {
    if (v === undefined || k === 'x' || k === 'y' || k === 'nodeId' || k === 'type') continue
    dataPatch[k] = v
  }
  if (!Object.keys(dataPatch).length) return
  ids.forEach(id => updateNodeData(id, dataPatch))
  triggerSave()
  recordHistory()
  refreshSelectionSnapshot()
}

function newNodeId(i = 0): string {
  return `node-${Date.now()}-${Math.floor(Math.random() * 1e6)}-${i}`
}

function newEdgeId(i = 0): string {
  return `e-${Date.now()}-${Math.floor(Math.random() * 1e6)}-${i}`
}

function selectOnly(ids: string[]) {
  if (getSelectedNodes.value.length) removeSelectedNodes(getSelectedNodes.value)
  const targets = ids.map(id => findNode(id)).filter((n): n is NonNullable<typeof n> => !!n)
  if (targets.length) addSelectedNodes(targets)
}

function cloneCnodes(ids: string[], offset: number): CNode[] {
  const idSet = new Set(ids)
  return extractCanvasNodes()
    .filter(c => idSet.has(c.node_id))
    .map((c, i) => ({ ...c, node_id: newNodeId(i), x: c.x + offset, y: c.y + offset }))
}

function duplicateSelection() {
  const clones = cloneCnodes(canvasStore.selectedNodeIds, 24)
  if (!clones.length) return
  addNodes(buildVfNodes(clones))
  triggerSave()
  recordHistory()
  nextTick(() => selectOnly(clones.map(c => c.node_id)))
}

// Clipboard for copy/paste (canvas-local).
let clipboardCnodes: CNode[] = []
function copySelection() {
  const ids = new Set(canvasStore.selectedNodeIds)
  clipboardCnodes = extractCanvasNodes().filter(c => ids.has(c.node_id))
}
function pasteClipboard() {
  if (!clipboardCnodes.length) return
  const clones = clipboardCnodes.map((c, i) => ({ ...c, node_id: newNodeId(i), x: c.x + 24, y: c.y + 24 }))
  addNodes(buildVfNodes(clones))
  triggerSave()
  recordHistory()
  nextTick(() => selectOnly(clones.map(c => c.node_id)))
}

function isEditableTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  return !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result ?? ''))
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(file)
  })
}

function measureImage(src: string): Promise<{ width: number; height: number }> {
  return new Promise(resolve => {
    const img = new Image()
    img.onload = () => resolve({ width: img.naturalWidth || PASTED_IMAGE_MAX_WIDTH, height: img.naturalHeight || PASTED_IMAGE_MAX_HEIGHT })
    img.onerror = () => resolve({ width: PASTED_IMAGE_MAX_WIDTH, height: PASTED_IMAGE_MAX_HEIGHT })
    img.src = src
  })
}

function fitPastedImageSize(width: number, height: number) {
  const safeWidth = width > 0 ? width : PASTED_IMAGE_MAX_WIDTH
  const safeHeight = height > 0 ? height : PASTED_IMAGE_MAX_HEIGHT
  const scale = Math.min(PASTED_IMAGE_MAX_WIDTH / safeWidth, PASTED_IMAGE_MAX_HEIGHT / safeHeight, 1)
  return {
    width: Math.max(80, Math.round(safeWidth * scale)),
    height: Math.max(60, Math.round(safeHeight * scale)),
  }
}

function pastedImagePosition(width: number, height: number) {
  const container = flowContainerRef.value
  const fallback = container?.getBoundingClientRect()
  const client = lastCanvasPointer.value ?? (fallback
    ? { x: fallback.left + fallback.width / 2, y: fallback.top + fallback.height / 2 }
    : { x: window.innerWidth / 2, y: window.innerHeight / 2 })
  const flowPos = screenToFlowCoordinate(client)
  return { x: flowPos.x - width / 2, y: flowPos.y - height / 2 }
}

async function addImageFileToCanvas(file: File) {
  if (!file.type.startsWith('image/')) return
  const src = await readFileAsDataUrl(file)
  const measured = await measureImage(src)
  const size = fitPastedImageSize(measured.width, measured.height)
  const position = pastedImagePosition(size.width, size.height)
  const nodeId = newNodeId()
  const newNode: VfNode = {
    id: nodeId,
    type: 'image',
    position,
    style: {
      width: `${size.width}px`,
      height: `${size.height}px`,
    },
    data: {
      nodeId,
      src,
      alt: file.name || 'Pasted image',
      width: size.width,
      height: size.height,
      cornerRadius: 8,
      opacity: 1,
    },
  }
  addNodes([newNode])
  activeTool.value = 'pointer'
  triggerSave()
  recordHistory()
  await nextTick()
  selectOnly([nodeId])
}

async function onCanvasPaste(e: ClipboardEvent) {
  if (!canvasStore.isShown || !canvasStore.currentCanvas || isEditableTarget(e.target)) return
  const items = Array.from(e.clipboardData?.items ?? [])
  const imageItem = items.find(item => item.kind === 'file' && item.type.startsWith('image/'))
  if (imageItem) {
    const file = imageItem.getAsFile()
    if (!file) return
    e.preventDefault()
    await addImageFileToCanvas(file)
    return
  }
  if (clipboardCnodes.length) {
    e.preventDefault()
    pasteClipboard()
  }
}

function deleteSelection() {
  const ids = canvasStore.selectedNodeIds
  if (!ids.length) return
  removeNodes(ids)
  triggerSave()
  recordHistory()
  canvasStore.setSelectedNode(null)
  canvasStore.setSelectedNodeIds([])
}

watch(
  () => canvasStore.pendingAction,
  (a) => {
    if (!a) return
    switch (a.type) {
      case 'align': alignNodes(a.payload as AlignDir); break
      case 'distribute': distributeNodes(a.payload as 'h' | 'v'); break
      case 'zorder': setZOrder(a.payload as 'front' | 'back'); break
      case 'batchPatch': batchPatch(a.payload as Partial<DrawNodeSnapshot>); break
      case 'duplicate': duplicateSelection(); break
      case 'copy': copySelection(); break
      case 'paste': pasteClipboard(); break
      case 'delete': deleteSelection(); break
    }
  }
)

// ── Keyboard shortcuts (canvas only) ──────────────────────────────────────────
function onCanvasKeydown(e: KeyboardEvent) {
  if (!canvasStore.isShown) return
  if (isEditableTarget(e.target)) return
  const meta = e.metaKey || e.ctrlKey
  const lower = e.key.toLowerCase()
  if (meta && lower === 'z' && !e.shiftKey) { e.preventDefault(); history.undo() }
  else if (meta && ((lower === 'z' && e.shiftKey) || lower === 'y')) { e.preventDefault(); history.redo() }
  else if (meta && (e.key === 'd' || e.key === 'D')) { e.preventDefault(); duplicateSelection() }
  else if (meta && (e.key === 'c' || e.key === 'C')) { copySelection() }
  else if (meta && e.key === ']') { e.preventDefault(); setZOrder('front') }
  else if (meta && e.key === '[') { e.preventDefault(); setZOrder('back') }
  else if ((e.key === 'Delete' || e.key === 'Backspace') && canvasStore.selectedNodeIds.length) {
    e.preventDefault(); deleteSelection()
  }
}

function onNodeDblClick(event: NodeMouseEvent) {
  if (event.node.type === 'text' || event.node.type === 'shape') {
    openAnnotationEditor(event.node.id)
    return
  }
  if (event.node.type === 'image' || event.node.type === 'line') return
  const paperId = event.node.data?.paperId as string | undefined
  if (paperId) openPaperById(paperId)
}

// ── Add paper to canvas ───────────────────────────────────────────────────────

function openPaperPicker() {
  pickerSearch.value = ''
  pickerError.value = ''
  showPaperPicker.value = true
}

// Pick a spot for a newly-added node that is inside the current viewport and not
// overlapping an existing node. Scans a grid spiralling out from the viewport
// centre and returns the first blank spot that still fits on screen, so the node
// never lands off-screen where the user has to pan around to find it.
function findBlankViewportPosition(): { x: number; y: number } {
  const NODE_W = 200, NODE_H = 130, GAP = 28, EDGE = 24

  let minX: number, minY: number, maxX: number, maxY: number
  const el = flowContainerRef.value
  if (el) {
    const rect = el.getBoundingClientRect()
    const tl = screenToFlowCoordinate({ x: rect.left, y: rect.top })
    const br = screenToFlowCoordinate({ x: rect.right, y: rect.bottom })
    minX = Math.min(tl.x, br.x); maxX = Math.max(tl.x, br.x)
    minY = Math.min(tl.y, br.y); maxY = Math.max(tl.y, br.y)
  } else {
    const vp = getViewport()
    const w = 800, h = 600
    minX = -vp.x / vp.zoom; maxX = (w - vp.x) / vp.zoom
    minY = -vp.y / vp.zoom; maxY = (h - vp.y) / vp.zoom
  }

  const centerX = (minX + maxX) / 2 - NODE_W / 2
  const centerY = (minY + maxY) / 2 - NODE_H / 2
  const loX = minX + EDGE, loY = minY + EDGE
  const hiX = maxX - EDGE - NODE_W, hiY = maxY - EDGE - NODE_H
  if (hiX <= loX || hiY <= loY) return { x: centerX, y: centerY }

  const others = nodes.value.map(n => ({
    x: n.position.x,
    y: n.position.y,
    w: n.dimensions?.width || (n.type === 'paper' ? NODE_W : 160),
    h: n.dimensions?.height || (n.type === 'paper' ? NODE_H : 90),
  }))
  const fits = (x: number, y: number) => others.every(o =>
    x + NODE_W + GAP <= o.x || x >= o.x + o.w + GAP ||
    y + NODE_H + GAP <= o.y || y >= o.y + o.h + GAP
  )

  const stepX = NODE_W / 2 + GAP
  const stepY = NODE_H / 2 + GAP
  const maxRing = Math.ceil(Math.max((maxX - minX) / stepX, (maxY - minY) / stepY))

  for (let r = 0; r <= maxRing; r++) {
    for (let dy = -r; dy <= r; dy++) {
      for (let dx = -r; dx <= r; dx++) {
        if (r > 0 && Math.max(Math.abs(dx), Math.abs(dy)) !== r) continue
        const x = centerX + dx * stepX
        const y = centerY + dy * stepY
        if (x < loX || x > hiX || y < loY || y > hiY) continue
        if (fits(x, y)) return { x, y }
      }
    }
  }
  return { x: centerX, y: centerY }
}

function addPaperToCanvas(paper: PaperIndexEntry) {
  // Adding a paper to the canvas isn't "reading" — don't touch recency.
  const cv = canvasStore.currentCanvas
  if (!cv) return
  const exists = nodes.value.some(n => n.data.paperId === paper.id)
  if (exists) {
    pickerError.value = t('canvas.alreadyAdded')
    return
  }
  const nodeId = `node-${Date.now()}`
  const { x, y } = findBlankViewportPosition()
  const newNode: VfNode = {
    id: nodeId,
    type: 'paper',
    position: { x, y },
    data: {
      title: paper.title,
      authors: paper.authors,
      year: paper.year,
      paperId: paper.id,
      nodeId,
      valid: true,
      noteTitles: noteTitlesMap.value.get(paper.id) ?? [],
    },
  }
  addNodes([newNode])
  showPaperPicker.value = false
  refreshPaperNoteTitles(paper.slug, paper.id).catch(() => {})
  triggerSave()
  recordHistory()
}

// ── Node hover tooltip ────────────────────────────────────────────────────────

function clearHoverTooltip() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
  hoverNodeId.value = null
  hoverContent.value = ''
  hoverLoading.value = false
}

function onNodeMouseEnter(event: NodeMouseEvent) {
  if (ctxMenu.value.show || isDraggingNode) {
    clearHoverTooltip()
    return
  }
  const nd = event.node
  if (nd.type !== 'paper') return
  const mouseEvt = event.event as MouseEvent
  hoverPos.value = { x: mouseEvt.clientX, y: mouseEvt.clientY }
  if (hoverTimer) clearTimeout(hoverTimer)
  hoverTimer = setTimeout(async () => {
    hoverTimer = null
    if (ctxMenu.value.show || isDraggingNode) return
    hoverNodeId.value = nd.id
    hoverLoading.value = true
    hoverContent.value = ''
    try {
      const raw = await canvasStore.getNodeDisplayContent(nd.data.paperId, 'notes')
      if (!ctxMenu.value.show && hoverNodeId.value === nd.id) {
        hoverContent.value = raw
      }
    } finally {
      if (hoverNodeId.value === nd.id) hoverLoading.value = false
    }
  }, 600)
}

function onNodeMouseLeave() {
  clearHoverTooltip()
}

const renderedHoverContent = computed(() => {
  if (!hoverContent.value) return ''
  return renderMarkdown(hoverContent.value)
})

// ── Context menu ──────────────────────────────────────────────────────────────

function onNodeContextMenu(event: NodeMouseEvent) {
  event.event.preventDefault()
  clearHoverTooltip()
  const paperId = (event.node.data?.paperId as string) ?? null

  ctxMenu.value = {
    show: true,
    x: (event.event as MouseEvent).clientX,
    y: (event.event as MouseEvent).clientY,
    nodeId: event.node.id,
    edgeId: null,
    paperId,
    nodeType: event.node.type ?? 'paper',
  }
}

function onEdgeContextMenu(event: EdgeMouseEvent) {
  event.event.preventDefault()
  clearHoverTooltip()
  ctxMenu.value = {
    show: true,
    x: (event.event as MouseEvent).clientX,
    y: (event.event as MouseEvent).clientY,
    nodeId: null,
    edgeId: event.edge.id,
    paperId: null,
    nodeType: null,
  }
}

function closeCtxMenu() {
  ctxMenu.value = { show: false, x: 0, y: 0, nodeId: null, edgeId: null, paperId: null, nodeType: null }
}

function ctxSelectPaper() {
  const paperId = ctxMenu.value.paperId
  closeCtxMenu()
  if (paperId) openPaperById(paperId)
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

// ── Node / edge style controls ───────────────────────────────────────────────

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

const ctxCurrentNodeColor = computed(() => {
  const id = ctxMenu.value.nodeId
  if (!id) return undefined as string | undefined
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (nodes.value.find(n => n.id === id)?.data as any)?.color as string | undefined
})

function ctxSetNodeColor(color: string | undefined) {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n => (
    n.id === nodeId
      ? { ...n, data: { ...n.data, color } }
      : n
  ))
  triggerSave()
  recordHistory()
}

// ── Text node style controls ──────────────────────────────────────────────────

const TEXT_FONT_SIZES = [
  { label: '小', value: 12 },
  { label: '中', value: 14 },
  { label: '大', value: 18 },
  { label: '特大', value: 24 },
]

const ctxCurrentTextData = computed(() => {
  const id = ctxMenu.value.nodeId
  if (!id || ctxMenu.value.nodeType !== 'text') return { fontSize: 14, bold: false, italic: false }
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const d = nodes.value.find(n => n.id === id)?.data as any
  return { fontSize: d?.fontSize ?? 14, bold: !!d?.bold, italic: !!d?.italic }
})

function ctxSetTextFontSize(size: number) {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n =>
    n.id === nodeId ? { ...n, data: { ...n.data, fontSize: size } } : n
  )
  triggerSave()
  recordHistory()
}

function ctxToggleTextBold() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n =>
    n.id === nodeId ? { ...n, data: { ...n.data, bold: !n.data.bold } } : n
  )
  triggerSave()
  recordHistory()
}

function ctxToggleTextItalic() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n =>
    n.id === nodeId ? { ...n, data: { ...n.data, italic: !n.data.italic } } : n
  )
  triggerSave()
  recordHistory()
}

const ctxCurrentEdgeData = computed(() => {
  const id = ctxMenu.value.edgeId
  if (!id) return { color: undefined as string | undefined, strokeWidth: 1.8 }
  const edge = edges.value.find(e => e.id === id)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const data = edge?.data as any
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const style = edge?.style as any
  return {
    color: (data?.edgeColor ?? style?.stroke) as string | undefined,
    strokeWidth: Number(data?.edgeStrokeWidth ?? style?.strokeWidth ?? 1.8),
  }
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

function ctxSetEdgeStrokeWidth(width: number) {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  edges.value = edges.value.map(e => (
    e.id === edgeId
      ? {
          ...e,
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          style: { ...(e.style as any), strokeWidth: width },
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          data: { ...(e.data as any), edgeStrokeWidth: width },
        }
      : e
  ))
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
    data: { edgeStrokeWidth: 1.8 },
  }
  addEdges([newEdge])
  triggerSave()
  recordHistory()
}

function acceptAllSuggestions(suggestions: SuggestedEdge[]) {
  suggestions.forEach((s, i) => acceptSuggestion(s, i))
}

// ── M10: Auto layout ──────────────────────────────────────────────────────────
async function applyLayout(layout: 'timeline' | 'topological', direction: 'horizontal' | 'vertical') {
  if (!canvasStore.currentCanvas) return
  if (!window.confirm(t('canvas.layoutConfirm'))) return
  layoutHistory.value = (nodes.value as any[]).map(n => ({
    node_id: n.id, x: n.position.x, y: n.position.y,
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

// ── Keyboard shortcuts ────────────────────────────────────────────────────────

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    closeCtxMenu()
    showPaperPicker.value = false
    editingEdgeId.value = null
    editingAnnotationId.value = null
    activeTool.value = 'pointer'
    resetShapeDraft()
  }
  // Tool shortcuts (only when not typing in an input)
  const tag = (e.target as HTMLElement)?.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA') return
  // Bare tool shortcuts must not fire when a modifier is held, otherwise 'v'
  // clobbers Cmd/Ctrl+V (paste) handled in onCanvasKeydown.
  if (e.metaKey || e.ctrlKey) return
  if (e.key === 'v' || e.key === 'V') {
    activeTool.value = 'pointer'
    resetShapeDraft()
  }
  if (e.key === 't' || e.key === 'T') selectTool('text')
  if (e.key === 'r' || e.key === 'R') selectTool('shape')
}

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

function onCanvasNotesUpdated(event: Event) {
  const slug = (event as CustomEvent<{ slug: string }>).detail?.slug
  if (!slug) return
  refreshPaperNoteTitles(slug).catch(() => {})
}

function onNotesUpdated(event: Event) {
  const slug = (event as CustomEvent<{ slug: string }>).detail?.slug
  if (!slug) return
  refreshPaperNoteTitles(slug).catch(() => {})
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

onMounted(async () => {
  if (!canvasStore.canvasList.length) {
    await canvasStore.loadList()
  }
  await canvasStore.loadSettings()
  document.addEventListener('keydown', onKeydown)
  document.addEventListener('keydown', onCanvasKeydown)
  document.addEventListener('paste', onCanvasPaste)
  document.addEventListener('pointerdown', onDocClick)
  window.addEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.addEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  window.addEventListener('argus-notes-updated', onNotesUpdated)
  window.addEventListener('argus-canvas-export-fit', onExportFit)
})

onUnmounted(() => {
  document.removeEventListener('keydown', onKeydown)
  document.removeEventListener('keydown', onCanvasKeydown)
  document.removeEventListener('paste', onCanvasPaste)
  document.removeEventListener('pointerdown', onDocClick)
  window.removeEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.removeEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  window.removeEventListener('argus-notes-updated', onNotesUpdated)
  window.removeEventListener('argus-canvas-export-fit', onExportFit)
  resetShapeDraft()
  if (hoverTimer) clearTimeout(hoverTimer)
})

// Refresh nodes when library papers change (validity)
watch(() => library.papers, () => {
  nodes.value = nodes.value.map(n => {
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
        noteTitles: paper ? (noteTitlesMap.value.get(paper.id) ?? n.data.noteTitles ?? []) : [],
      },
    }
  })
  const cv = canvasStore.currentCanvas
  if (cv) loadNoteTitles(cv.nodes)
})
</script>

<template>
  <div class="canvas-panel">
    <!-- Toolbar -->
    <div class="canvas-toolbar">
      <button class="back-btn" :title="t('canvas.backToList')" @click="emit('close')">
        <Icon icon="fluent:chevron-left-24-regular" width="13" height="13" />
      </button>
      <span class="canvas-name">{{ canvasStore.currentCanvas?.name || t('canvas.noCanvases') }}</span>
      <div class="toolbar-actions">
        <!-- Suggest Edges -->
        <button
          v-if="canvasStore.currentCanvas"
          class="tb-action-btn"
          :class="{ 'tb-action-btn--active': showSuggestPanel }"
          @click="showSuggestPanel = !showSuggestPanel"
        >
          <Icon icon="fluent:share-android-24-regular" width="13" height="13" />
          {{ t('canvas.suggestEdges') }}
        </button>

        <!-- Auto Layout dropdown -->
        <div v-if="canvasStore.currentCanvas" class="layout-wrap">
          <button class="tb-action-btn" @click="showLayoutMenu = !showLayoutMenu" :disabled="applyingLayout">
            <Icon icon="fluent:organization-24-regular" width="13" height="13" />
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
        >
          <Icon icon="fluent:arrow-download-24-regular" width="13" height="13" />
          {{ t('canvas.exportImage') }}
        </button>

        <!-- Add Paper -->
        <button
          v-if="canvasStore.currentCanvas"
          class="tb-action-btn tb-action-btn--accent"
          @click="openPaperPicker"
        >
          <Icon icon="fluent:add-24-regular" width="13" height="13" />
          {{ t('canvas.addPaper') }}
        </button>
      </div>
    </div>

    <!-- Canvas area -->
    <div class="canvas-content">
      <div
        v-if="canvasStore.currentCanvas"
        class="flow-wrap"
        ref="flowContainerRef"
        @pointermove="lastCanvasPointer = { x: $event.clientX, y: $event.clientY }"
        @pointerdown="onFlowPointerDown"
      >
        <VueFlow
          v-model:nodes="nodes"
          v-model:edges="edges"
          :node-types="nodeTypes"
          :edge-types="edgeTypes"
          :connection-mode="ConnectionMode.Loose"
          :default-edge-options="{ type: 'adjustable', markerEnd: MarkerType.ArrowClosed }"
          :snap-to-grid="false"
          :pan-on-drag="pointerPanOnDrag"
          :selection-key-code="pointerSelectionKeyCode"
          :elements-selectable="activeTool === 'pointer'"
          :select-nodes-on-drag="activeTool === 'pointer'"
          :nodes-draggable="activeTool === 'pointer'"
          fit-view-on-init
          class="canvas-flow"
          :class="{ 'tool-text': activeTool === 'text', 'tool-shape': activeTool === 'shape' || activeTool === 'line' }"
          @node-click="onNodeClick"
          @node-double-click="onNodeDblClick"
          @node-mouse-enter="onNodeMouseEnter"
          @node-mouse-leave="onNodeMouseLeave"
          @node-context-menu="onNodeContextMenu"
          @edge-context-menu="onEdgeContextMenu"
          @nodes-change="triggerSave"
          @edges-change="triggerSave"
          @pane-click="onPaneClick"
        >
          <Background pattern-color="var(--border-subtle, #e5e7eb)" :gap="24" />
          <Controls />
          <MiniMap class="canvas-minimap" />
        </VueFlow>

        <div
          v-if="shapeDraft"
          class="shape-draft-preview"
          :style="shapePreviewStyle"
        />

        <!-- Smart alignment guides -->
        <div class="snap-guides">
          <div
            v-for="(gd, i) in snapGuides"
            :key="i"
            class="snap-guide"
            :class="gd.vertical ? 'snap-v' : 'snap-h'"
            :style="gd.vertical ? { left: `${gd.pos}px` } : { top: `${gd.pos}px` }"
          />
        </div>

        <!-- Bottom center toolbar -->
        <div class="canvas-bottom-toolbar">
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'pointer', 'pan-mode': isPointerPanMode }"
            :title="pointerToolTitle"
            @click="selectPointerTool"
          >
            <Icon v-if="pointerDragMode === 'select'" icon="fluent:cursor-24-regular" width="15" height="15" />
            <Icon v-else icon="fluent:hand-right-24-regular" width="15" height="15" />
          </button>
          <div class="btb-sep" />
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'text' }"
            title="文字 (T)"
            @click="selectTool('text')"
          >
            <Icon icon="fluent:text-t-24-regular" width="15" height="15" />
          </button>
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'shape' && pendingShapeKind === 'rect' }"
            title="矩形 (R)"
            @click="selectShape('rect')"
          >
            <Icon icon="fluent:rectangle-landscape-24-regular" width="15" height="15" />
          </button>
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'shape' && pendingShapeKind === 'ellipse' }"
            title="椭圆"
            @click="selectShape('ellipse')"
          >
            <Icon icon="fluent:oval-24-regular" width="15" height="15" />
          </button>
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'shape' && pendingShapeKind === 'diamond' }"
            title="菱形"
            @click="selectShape('diamond')"
          >
            <Icon icon="fluent:diamond-24-regular" width="15" height="15" />
          </button>
          <div class="btb-sep" />
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'line' && pendingLineKind === 'line' }"
            title="直线"
            @click="selectLine('line')"
          >
            <Icon icon="fluent:line-24-regular" width="15" height="15" />
          </button>
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'line' && pendingLineKind === 'arrow' }"
            title="箭头"
            @click="selectLine('arrow')"
          >
            <Icon icon="fluent:arrow-up-right-24-regular" width="15" height="15" />
          </button>
        </div>

        <!-- Hover tooltip -->
        <Teleport to="body">
          <div
            v-if="hoverNodeId && (hoverLoading || hoverContent)"
            class="hover-tooltip"
            :style="{ left: `${hoverPos.x}px`, top: `${hoverPos.y}px` }"
          >
            <div v-if="hoverLoading" class="hover-loading">{{ t('canvas.hoverLoading') }}</div>
            <div v-else-if="hoverContent" class="hover-content" v-html="renderedHoverContent" />
            <div v-else class="hover-empty">{{ t('canvas.noHoverContent') }}</div>
          </div>
        </Teleport>
      </div>

      <!-- No canvas selected -->
      <div v-if="!canvasStore.currentCanvas" class="no-canvas-selected">
        <Icon icon="fluent:share-android-24-regular" width="48" height="48" style="opacity:0.2" />
        <p>{{ t('canvas.noCanvases') }}</p>
        <p class="hint">{{ t('canvas.noCanvasesHint') }}</p>
      </div>

      <!-- Suggest Panel -->
      <SuggestPanel
        v-if="showSuggestPanel && canvasStore.currentCanvas"
        :canvas-id="canvasStore.currentCanvas.id"
        :paper-names="paperNames"
        @accept="acceptSuggestion"
        @accept-all="acceptAllSuggestions"
        @close="showSuggestPanel = false"
      />
    </div>

    <!-- Paper picker dialog -->
    <Teleport to="body">
      <div v-if="showPaperPicker" class="modal-overlay" @click.self="showPaperPicker = false">
        <div class="picker-dialog">
          <div class="picker-header">
            <span class="picker-title">{{ t('canvas.addPaperTitle') }}</span>
            <button class="close-btn" @click="showPaperPicker = false">
              <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
            </button>
          </div>
          <input v-model="pickerSearch" class="picker-search" :placeholder="t('canvas.searchPapers')" autofocus />
          <p v-if="pickerError" class="picker-error">{{ pickerError }}</p>
          <div class="picker-list">
            <div v-if="filteredPapers.length === 0" class="picker-empty">{{ t('canvas.noPapers') }}</div>
            <div
              v-for="paper in filteredPapers"
              :key="paper.id"
              class="picker-item"
              :class="{ 'picker-item--taken': nodes.some(n => n.data.paperId === paper.id) }"
              @click="addPaperToCanvas(paper)"
            >
              <div class="picker-item-title">{{ paper.title }}</div>
              <div class="picker-item-meta">
                <span>{{ paper.authors.slice(0, 2).join(', ') }}{{ paper.authors.length > 2 ? ' 等' : '' }}</span>
                <span v-if="paper.year">{{ paper.year }}</span>
              </div>
              <span v-if="nodes.some(n => n.data.paperId === paper.id)" class="picker-item-badge">已添加</span>
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
        <template v-if="ctxMenu.nodeId">

          <!-- Text node -->
          <template v-if="ctxMenu.nodeType === 'text'">
            <div class="ctx-style-section">
              <span class="ctx-style-label">字号</span>
              <div class="ctx-width-row">
                <button v-for="sz in TEXT_FONT_SIZES" :key="sz.value"
                  class="ctx-width-btn"
                  :class="{ 'ctx-width-btn--active': ctxCurrentTextData.fontSize === sz.value }"
                  @click.stop="ctxSetTextFontSize(sz.value)">{{ sz.label }}</button>
              </div>
            </div>
            <div class="ctx-style-section">
              <span class="ctx-style-label">样式</span>
              <div class="ctx-width-row">
                <button class="ctx-width-btn ctx-font-btn" style="font-weight:700"
                  :class="{ 'ctx-width-btn--active': ctxCurrentTextData.bold }"
                  @click.stop="ctxToggleTextBold">B</button>
                <button class="ctx-width-btn ctx-font-btn" style="font-style:italic"
                  :class="{ 'ctx-width-btn--active': ctxCurrentTextData.italic }"
                  @click.stop="ctxToggleTextItalic">I</button>
              </div>
            </div>
            <div class="ctx-style-section">
              <span class="ctx-style-label">颜色</span>
              <div class="ctx-color-row">
                <button v-for="color in COLOR_PALETTE" :key="String(color.value)"
                  class="ctx-color-swatch"
                  :class="{ 'ctx-color-swatch--active': ctxCurrentNodeColor === color.value }"
                  :title="color.label" :style="color.value ? { background: color.value } : {}"
                  @click.stop="ctxSetNodeColor(color.value)">
                  <template v-if="!color.value">
                    <Icon icon="fluent:prohibited-24-regular" width="12" height="12" style="color: var(--accent)" />
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="openAnnotationEditor(ctxMenu.nodeId!); closeCtxMenu()">
              <Icon icon="fluent:edit-24-regular" width="12" height="12" />
              编辑内容
            </button>
            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <Icon icon="fluent:delete-24-regular" width="12" height="12" />
              删除
            </button>
          </template>

          <!-- Shape node -->
          <template v-else-if="ctxMenu.nodeType === 'shape'">
            <div class="ctx-style-section">
              <span class="ctx-style-label">边框颜色</span>
              <div class="ctx-color-row">
                <button v-for="color in COLOR_PALETTE" :key="String(color.value)"
                  class="ctx-color-swatch"
                  :class="{ 'ctx-color-swatch--active': ctxCurrentNodeColor === color.value }"
                  :title="color.label" :style="color.value ? { background: color.value } : {}"
                  @click.stop="ctxSetNodeColor(color.value)">
                  <template v-if="!color.value">
                    <Icon icon="fluent:prohibited-24-regular" width="12" height="12" style="color: var(--accent)" />
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="openAnnotationEditor(ctxMenu.nodeId!); closeCtxMenu()">
              <Icon icon="fluent:edit-24-regular" width="12" height="12" />
              编辑标签
            </button>
            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <Icon icon="fluent:delete-24-regular" width="12" height="12" />
              删除
            </button>
          </template>

          <!-- Image node -->
          <template v-else-if="ctxMenu.nodeType === 'image'">
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <Icon icon="fluent:delete-24-regular" width="12" height="12" />
              删除图片
            </button>
          </template>

          <!-- Paper node -->
          <template v-else>
            <div class="ctx-style-section">
              <span class="ctx-style-label">边框颜色</span>
              <div class="ctx-color-row">
                <button v-for="color in COLOR_PALETTE" :key="String(color.value)"
                  class="ctx-color-swatch"
                  :class="{ 'ctx-color-swatch--active': ctxCurrentNodeColor === color.value }"
                  :title="color.label" :style="color.value ? { background: color.value } : {}"
                  @click.stop="ctxSetNodeColor(color.value)">
                  <template v-if="!color.value">
                    <Icon icon="fluent:prohibited-24-regular" width="12" height="12" style="color: var(--accent)" />
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="ctxSelectPaper">
              <Icon icon="fluent:book-24-regular" width="12" height="12" />
              {{ t('canvas.viewPaper') }}
            </button>

            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <Icon icon="fluent:delete-24-regular" width="12" height="12" />
              {{ t('canvas.removePaper') }}
            </button>
          </template>
        </template>
        <template v-if="ctxMenu.edgeId">
          <div class="ctx-style-section">
            <span class="ctx-style-label">颜色</span>
            <div class="ctx-color-row">
              <button
                v-for="color in COLOR_PALETTE"
                :key="String(color.value)"
                class="ctx-color-swatch"
                :class="{ 'ctx-color-swatch--active': ctxCurrentEdgeData.color === color.value }"
                :title="color.label"
                :style="color.value ? { background: color.value } : {}"
                @click.stop="ctxSetEdgeColor(color.value)"
              >
                <template v-if="!color.value">
                  <Icon icon="fluent:prohibited-24-regular" width="12" height="12" style="color: var(--accent)" />
                </template>
              </button>
            </div>
          </div>
          <div class="ctx-style-section">
            <span class="ctx-style-label">粗细</span>
            <div class="ctx-width-row">
              <button
                v-for="width in EDGE_WIDTHS"
                :key="width.value"
                class="ctx-width-btn"
                :class="{ 'ctx-width-btn--active': Math.abs(ctxCurrentEdgeData.strokeWidth - width.value) < 0.1 }"
                @click.stop="ctxSetEdgeStrokeWidth(width.value)"
              >
                {{ width.label }}
              </button>
            </div>
          </div>
          <div class="ctx-divider" />
          <button class="ctx-item" @click="ctxEditEdgeLabel">
            <Icon icon="fluent:edit-24-regular" width="12" height="12" />
            {{ t('canvas.edgeLabelEdit') }}
          </button>
          <div class="ctx-divider" />
          <button class="ctx-item ctx-item--danger" @click="ctxRemoveEdge">
            <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
            {{ t('canvas.removeEdge') }}
          </button>
        </template>
      </div>
    </Teleport>

    <!-- Edge label editor -->
    <Teleport to="body">
      <div v-if="editingEdgeId" class="modal-overlay" @click.self="editingEdgeId = null">
        <div class="edge-label-dialog">
          <p class="edge-label-title">{{ t('canvas.edgeLabelEdit') }}</p>
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

    <!-- Text / Shape node content editor -->
    <Teleport to="body">
      <div v-if="editingAnnotationId" class="modal-overlay" @click.self="editingAnnotationId = null">
        <div class="edge-label-dialog">
          <p class="edge-label-title">编辑内容</p>
          <textarea
            v-model="editingAnnotationContent"
            class="annotation-textarea"
            placeholder="输入内容..."
            rows="4"
            autofocus
            @keydown.escape="editingAnnotationId = null"
          />
          <div class="edge-label-actions">
            <button class="btn-ghost-sm" @click="editingAnnotationId = null">取消</button>
            <button class="btn-accent-sm" @click="commitAnnotationEdit">确定</button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.canvas-panel {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: var(--bg-primary);
}

.canvas-toolbar {
  height: 40px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  padding: 0 16px;
  gap: 10px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}

.back-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 6px;
  color: var(--text-secondary);
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.back-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.canvas-name {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
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
  border: 1px solid var(--border-default);
  border-radius: 6px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  transition: background 0.12s, color 0.12s;
  white-space: nowrap;
}
.tb-action-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.tb-action-btn--accent {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}
.tb-action-btn--accent:hover { opacity: 0.88; }
.tb-action-btn--active {
  background: var(--accent-light) !important;
  color: var(--accent) !important;
}

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

.canvas-flow {
  width: 100%;
  height: 100%;
}

.shape-draft-preview {
  position: absolute;
  z-index: 6;
  pointer-events: none;
  border: 2px solid var(--accent);
  border-radius: 6px;
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 18%, transparent);
}

/* Smart alignment guides */
.snap-guides {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 7;
  overflow: hidden;
}
.snap-guide {
  position: absolute;
  background: #f0398b;
}
.snap-guide.snap-v { top: 0; bottom: 0; width: 1px; }
.snap-guide.snap-h { left: 0; right: 0; height: 1px; }

.layout-wrap { position: relative; }

.layout-menu {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  min-width: 200px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg, 10px);
  box-shadow: 0 4px 16px rgba(0,0,0,0.15);
  padding: 6px;
  z-index: 100;
}
.layout-menu-section {
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary);
  padding: 3px 8px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.layout-menu-sep { height: 1px; background: var(--border-subtle); margin: 4px 0; }
.layout-menu-item {
  display: block;
  width: 100%;
  padding: 6px 10px;
  font-size: 12px;
  color: var(--text-primary);
  text-align: left;
  border-radius: 4px;
  transition: background 0.1s;
}
.layout-menu-item:hover { background: var(--bg-hover); }
.layout-menu-item--undo { color: var(--accent); }

.no-canvas-selected {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-secondary);
  font-size: 14px;
}
.no-canvas-selected .hint {
  font-size: 12px;
  color: var(--text-tertiary);
}

/* Vue Flow overrides */
:deep(.vue-flow__edge-path) { stroke: var(--accent); }
:deep(.vue-flow__controls) { border-radius: 8px; overflow: hidden; }
:deep(.vue-flow__minimap) { border-radius: 8px; overflow: hidden; }
.canvas-minimap { bottom: 12px; right: 12px; }

/* Hover tooltip */
.hover-tooltip {
  position: fixed;
  z-index: 9999;
  width: 360px;
  max-height: 400px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.15);
  overflow-y: auto;
  padding: 14px 16px;
  pointer-events: none;
  transform: translate(16px, -50%);
}
.hover-loading { color: var(--text-tertiary); font-size: 12px; }
.hover-empty { color: var(--text-tertiary); font-size: 12px; font-style: italic; }
.hover-content { font-size: 12px; line-height: 1.6; color: var(--text-primary); }
:deep(.hover-content h1) { font-size: 15px; margin: 8px 0 4px; }
:deep(.hover-content h2) { font-size: 13px; margin: 6px 0 3px; }
:deep(.hover-content p) { margin: 4px 0; }
:deep(.hover-content ul, .hover-content ol) { padding-left: 16px; margin: 4px 0; }
:deep(.hover-content code) {
  background: var(--bg-secondary);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 11px;
}

/* Paper picker */
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
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
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
.picker-title { font-size: 14px; font-weight: 600; color: var(--text-primary); }
.close-btn {
  width: 24px; height: 24px;
  border-radius: 6px;
  display: flex; align-items: center; justify-content: center;
  color: var(--text-tertiary);
  transition: background 0.1s;
}
.close-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.picker-search {
  margin: 10px 16px;
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  width: calc(100% - 32px);
  box-sizing: border-box;
}
.picker-search:focus { outline: none; border-color: var(--accent); background: var(--bg-primary); }
.picker-error { color: #ef4444; font-size: 12px; margin: 0 16px 8px; }
.picker-list { flex: 1; overflow-y: auto; padding: 0 8px 12px; }
.picker-empty { text-align: center; color: var(--text-tertiary); font-size: 13px; padding: 24px; }
.picker-item {
  position: relative;
  padding: 9px 10px;
  border-radius: 7px;
  cursor: pointer;
  transition: background 0.1s;
}
.picker-item:hover { background: var(--bg-hover); }
.picker-item--taken { opacity: 0.5; pointer-events: none; }
.picker-item-title { font-size: 13px; font-weight: 500; color: var(--text-primary); line-height: 1.3; margin-bottom: 3px; }
.picker-item-meta { display: flex; gap: 10px; font-size: 11px; color: var(--text-tertiary); }
.picker-item-badge {
  position: absolute; right: 10px; top: 50%; transform: translateY(-50%);
  background: var(--accent-light); color: var(--accent);
  font-size: 10px; font-weight: 600; padding: 2px 6px; border-radius: 4px;
}

/* Context menu */
.canvas-ctx-menu {
  position: fixed;
  z-index: 9500;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0,0,0,0.15);
  padding: 4px 0;
  min-width: 170px;
}
.ctx-item {
  display: flex; align-items: center; gap: 8px;
  width: 100%; padding: 7px 14px; font-size: 13px;
  color: var(--text-primary); text-align: left;
  transition: background 0.1s;
}
.ctx-item:hover { background: var(--bg-hover); }
.ctx-item--danger { color: #ef4444; }
.ctx-item--danger:hover { background: #fee2e2; }
.ctx-style-section {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
}
.ctx-style-label {
  width: 52px;
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-tertiary);
}
.ctx-color-row {
  display: flex;
  align-items: center;
  gap: 5px;
}
.ctx-color-swatch {
  width: 18px;
  height: 18px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  border-radius: 50%;
  border: 1.5px solid transparent;
  background: var(--bg-secondary);
  transition: transform 0.1s, border-color 0.1s;
}
.ctx-color-swatch:hover { transform: scale(1.15); }
.ctx-color-swatch--active { border-color: var(--text-primary); }
.ctx-width-row {
  display: flex;
  gap: 4px;
}
.ctx-width-btn {
  padding: 2px 9px;
  border: 1px solid var(--border-default);
  border-radius: 4px;
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 11px;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.ctx-width-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.ctx-width-btn--active {
  border-color: var(--accent);
  background: var(--accent-light);
  color: var(--accent);
}
.ctx-font-btn {
  width: 28px;
  padding: 2px 0;
  text-align: center;
  font-size: 13px;
}
.ctx-divider { height: 1px; background: var(--border-subtle); margin: 3px 0; }

/* Edge label dialog */
.edge-label-dialog {
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.15);
  padding: 16px; width: 320px;
  display: flex; flex-direction: column; gap: 10px;
}
.edge-label-title { font-size: 13px; font-weight: 600; color: var(--text-primary); margin: 0; }
.edge-label-input {
  padding: 7px 10px; font-size: 13px;
  border: 1px solid var(--border-default); border-radius: 7px;
  background: var(--bg-secondary); color: var(--text-primary);
}
.edge-label-input:focus { outline: none; border-color: var(--accent); }
.edge-label-actions { display: flex; justify-content: flex-end; gap: 8px; }
.btn-ghost-sm {
  padding: 5px 12px; font-size: 12px; font-weight: 500;
  color: var(--text-secondary); border: 1px solid var(--border-default);
  border-radius: 6px; transition: background 0.1s;
}
.btn-ghost-sm:hover { background: var(--bg-hover); }
.btn-accent-sm {
  padding: 5px 14px; font-size: 12px; font-weight: 600;
  background: var(--accent); color: #fff; border-radius: 6px;
  transition: opacity 0.12s;
}
.btn-accent-sm:hover { opacity: 0.85; }

/* Annotation textarea */
.annotation-textarea {
  padding: 8px 10px; font-size: 13px; line-height: 1.5;
  border: 1px solid var(--border-default); border-radius: 7px;
  background: var(--bg-secondary); color: var(--text-primary);
  resize: vertical; min-height: 80px;
  font-family: inherit;
}
.annotation-textarea:focus { outline: none; border-color: var(--accent); }

/* Bottom center toolbar */
.canvas-bottom-toolbar {
  position: absolute;
  bottom: 20px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 2px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: 12px;
  padding: 5px 8px;
  box-shadow: 0 2px 12px rgba(0,0,0,0.12);
  z-index: 10;
  pointer-events: all;
}

.btb-sep {
  width: 1px;
  height: 20px;
  background: var(--border-subtle);
  margin: 0 4px;
}

.btb-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  color: var(--text-tertiary);
  background: transparent;
  transition: background 0.1s, color 0.1s;
}
.btb-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.btb-btn.active {
  background: var(--accent-light);
  color: var(--accent);
}
.btb-btn.pan-mode {
  background: color-mix(in srgb, var(--accent) 12%, var(--bg-primary));
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 26%, transparent);
}

/* Cursor changes when tool is active */
:deep(.tool-text .vue-flow__pane) { cursor: text; }
:deep(.tool-shape .vue-flow__pane) { cursor: crosshair; }
</style>
