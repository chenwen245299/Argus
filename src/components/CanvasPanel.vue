<script setup lang="ts">
import {
  ref, computed, markRaw, onMounted, onUnmounted, nextTick, watch,
} from 'vue'
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
import { marked } from 'marked'
import { useCanvasStore } from '../stores/canvas'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import { recordPaperAccess, sortPapersByRecentAccess } from '../utils/recentPapers'
import PaperNode from './canvas/PaperNode.vue'
import AdjustableEdge from './canvas/AdjustableEdge.vue'
import TextNode from './canvas/TextNode.vue'
import ShapeNode from './canvas/ShapeNode.vue'
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

const nodeTypes = markRaw({ paper: PaperNode, text: TextNode, shape: ShapeNode })
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
  getViewport,
  setViewport,
  fitView,
  screenToFlowCoordinate,
} = useVueFlow()

// ── Bottom toolbar tool state ─────────────────────────────────────────────────
type CanvasTool = 'pointer' | 'text' | 'shape'
type PointerDragMode = 'select' | 'pan'
const activeTool = ref<CanvasTool>('pointer')
const pointerDragMode = ref<PointerDragMode>('select')
const DEFAULT_SHAPE_WIDTH = 160
const DEFAULT_SHAPE_HEIGHT = 100
const MIN_SHAPE_SIZE = 12

type ShapeDraft = {
  startClientX: number
  startClientY: number
  currentClientX: number
  currentClientY: number
}
const shapeDraft = ref<ShapeDraft | null>(null)
const flowContainerRef = ref<HTMLElement | null>(null)
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
  if (activeTool.value !== 'shape') resetShapeDraft()
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
}

// ── Pane interactions → place text / draw shape node ─────────────────────────
function onPaneClick(event: MouseEvent) {
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
    },
  }
  addNodes([newNode])
  triggerSave()
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
  resetShapeDraft()
  addShapeFromDraft(draft)
  activeTool.value = 'pointer'
}

function onFlowPointerDown(event: PointerEvent) {
  if (activeTool.value !== 'shape' || event.button !== 0 || !canStartShapeDraft(event)) return
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
        data: {
          content: cn.content ?? '',
          color: cn.color,
          fontSize: cn.font_size,
          bold: cn.font_bold,
          italic: cn.font_italic,
          nodeId: cn.node_id,
        },
      } satisfies VfNode
    }
    if (nt === 'shape') {
      return {
        id: cn.node_id,
        type: 'shape',
        position: { x: cn.x, y: cn.y },
        data: {
          content: cn.content ?? '',
          color: cn.color,
          nodeId: cn.node_id,
          width: cn.width,
          height: cn.height,
        },
      } satisfies VfNode
    }
    const paper = paperById(cn.paper_id)
    return {
      id: cn.node_id,
      type: 'paper',
      position: { x: cn.x, y: cn.y },
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
    if (nt === 'text' || nt === 'shape') {
      return {
        node_id: n.id,
        paper_id: '',
        x: n.position.x,
        y: n.position.y,
        color: d.color as string | undefined,
        hover_source: undefined,
        node_type: nt,
        content: d.content as string | undefined,
        font_size: nt === 'text' ? (d.fontSize as number | undefined) : undefined,
        font_bold: nt === 'text' ? (d.bold as boolean | undefined) : undefined,
        font_italic: nt === 'text' ? (d.italic as boolean | undefined) : undefined,
        width: nt === 'shape' && Number.isFinite(d.width) ? d.width : undefined,
        height: nt === 'shape' && Number.isFinite(d.height) ? d.height : undefined,
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
}

// Watch for canvas switches (LeftSidebar calls canvasStore.openCanvas)
watch(
  () => canvasStore.currentCanvas?.id,
  async (id) => {
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
    id: `e-${Date.now()}`,
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
})

onNodeDragStop(() => { triggerSave() })

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
  if (event.node.type === 'text' || event.node.type === 'shape') return
  const paperId = event.node.data?.paperId as string | undefined
  if (paperId) selectPaperById(paperId)
}

function onNodeDblClick(event: NodeMouseEvent) {
  if (event.node.type === 'text' || event.node.type === 'shape') {
    openAnnotationEditor(event.node.id)
    return
  }
  const paperId = event.node.data?.paperId as string | undefined
  if (paperId) openPaperById(paperId)
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
  const exists = nodes.value.some(n => n.data.paperId === paper.id)
  if (exists) {
    pickerError.value = t('canvas.alreadyAdded')
    return
  }
  const nodeId = `node-${Date.now()}`
  const x = 100 + (nodes.value.length % 5) * 240
  const y = 100 + Math.floor(nodes.value.length / 5) * 180
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
}

// ── Node hover tooltip ────────────────────────────────────────────────────────

function clearHoverTooltip() {
  if (hoverTimer) { clearTimeout(hoverTimer); hoverTimer = null }
  hoverNodeId.value = null
  hoverContent.value = ''
  hoverLoading.value = false
}

function onNodeMouseEnter(event: NodeMouseEvent) {
  if (ctxMenu.value.show) {
    clearHoverTooltip()
    return
  }
  const nd = event.node
  const mouseEvt = event.event as MouseEvent
  hoverPos.value = { x: mouseEvt.clientX, y: mouseEvt.clientY }
  if (hoverTimer) clearTimeout(hoverTimer)
  hoverTimer = setTimeout(async () => {
    hoverTimer = null
    if (ctxMenu.value.show) return
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
  return marked(hoverContent.value) as string
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
}

function ctxRemoveEdge() {
  const edgeId = ctxMenu.value.edgeId
  if (!edgeId) return
  removeEdges([edgeId])
  closeCtxMenu()
  triggerSave()
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
}

function ctxToggleTextBold() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n =>
    n.id === nodeId ? { ...n, data: { ...n.data, bold: !n.data.bold } } : n
  )
  triggerSave()
}

function ctxToggleTextItalic() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  nodes.value = nodes.value.map(n =>
    n.id === nodeId ? { ...n, data: { ...n.data, italic: !n.data.italic } } : n
  )
  triggerSave()
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
}

// ── M10: Accept suggestion as real edge ───────────────────────────────────────
function acceptSuggestion(s: SuggestedEdge) {
  const fromNode = (nodes.value as any[]).find(n => n.data?.paperId === s.from_paper_id)
  const toNode = (nodes.value as any[]).find(n => n.data?.paperId === s.to_paper_id)
  if (!fromNode || !toNode) return
  const newEdge = {
    id: `edge-${Date.now()}`,
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
}

function acceptAllSuggestions(suggestions: SuggestedEdge[]) {
  suggestions.forEach(s => acceptSuggestion(s))
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
  document.addEventListener('pointerdown', onDocClick)
  window.addEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.addEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  window.addEventListener('argus-notes-updated', onNotesUpdated)
})

onUnmounted(() => {
  document.removeEventListener('keydown', onKeydown)
  document.removeEventListener('pointerdown', onDocClick)
  window.removeEventListener('argus-canvas-edge-control-changed', onEdgeControlChanged)
  window.removeEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  window.removeEventListener('argus-notes-updated', onNotesUpdated)
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
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <polyline points="15 18 9 12 15 6"/>
        </svg>
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
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M9.5 14.5 L14.5 9.5"/><circle cx="7" cy="17" r="2.5"/><circle cx="17" cy="7" r="2.5"/>
            <circle cx="17" cy="17" r="2.5"/>
          </svg>
          {{ t('canvas.suggestEdges') }}
        </button>

        <!-- Auto Layout dropdown -->
        <div v-if="canvasStore.currentCanvas" class="layout-wrap">
          <button class="tb-action-btn" @click="showLayoutMenu = !showLayoutMenu" :disabled="applyingLayout">
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

    <!-- Canvas area -->
    <div class="canvas-content">
      <div
        v-if="canvasStore.currentCanvas"
        class="flow-wrap"
        ref="flowContainerRef"
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
          :class="{ 'tool-text': activeTool === 'text', 'tool-shape': activeTool === 'shape' }"
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

        <!-- Bottom center toolbar -->
        <div class="canvas-bottom-toolbar">
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'pointer', 'pan-mode': isPointerPanMode }"
            :title="pointerToolTitle"
            @click="selectPointerTool"
          >
            <svg v-if="pointerDragMode === 'select'" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M5 3l14 9-7 1-4 7z"/>
            </svg>
            <svg v-else width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M18 11V7a2 2 0 0 0-4 0v4"/>
              <path d="M14 10V6a2 2 0 0 0-4 0v7"/>
              <path d="M10 12V8a2 2 0 1 0-4 0v7"/>
              <path d="M6 15c0 4 2.5 6 6.5 6H14c3 0 5-2 5-5v-5a2 2 0 0 0-4 0v1"/>
            </svg>
          </button>
          <div class="btb-sep" />
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'text' }"
            title="文字 (T)"
            @click="selectTool('text')"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 7V4h16v3"/><line x1="12" y1="4" x2="12" y2="20"/>
              <line x1="8" y1="20" x2="16" y2="20"/>
            </svg>
          </button>
          <button
            class="btb-btn"
            :class="{ active: activeTool === 'shape' }"
            title="矩形 (R)"
            @click="selectTool('shape')"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <rect x="3" y="5" width="18" height="14" rx="2"/>
            </svg>
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
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
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
                    <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                      <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                      <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                    </svg>
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="openAnnotationEditor(ctxMenu.nodeId!); closeCtxMenu()">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
              </svg>
              编辑内容
            </button>
            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M9 6V4h6v2"/>
              </svg>
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
                    <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                      <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                      <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                    </svg>
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="openAnnotationEditor(ctxMenu.nodeId!); closeCtxMenu()">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
              </svg>
              编辑标签
            </button>
            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M9 6V4h6v2"/>
              </svg>
              删除
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
                    <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                      <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                      <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                    </svg>
                  </template>
                </button>
              </div>
            </div>
            <div class="ctx-divider" />
            <button class="ctx-item" @click="ctxSelectPaper">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
                <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
              </svg>
              {{ t('canvas.viewPaper') }}
            </button>

            <div class="ctx-divider" />
            <button class="ctx-item ctx-item--danger" @click="ctxRemoveNode">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14H6L5 6"/><path d="M9 6V4h6v2"/>
              </svg>
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
                  <svg width="12" height="12" viewBox="0 0 16 16" fill="none">
                    <circle cx="8" cy="8" r="6" stroke="var(--accent)" stroke-width="1.5"/>
                    <line x1="3" y1="3" x2="13" y2="13" stroke="var(--accent)" stroke-width="1.5"/>
                  </svg>
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
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
              <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
            </svg>
            {{ t('canvas.edgeLabelEdit') }}
          </button>
          <div class="ctx-divider" />
          <button class="ctx-item ctx-item--danger" @click="ctxRemoveEdge">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
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
  height: 44px;
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
