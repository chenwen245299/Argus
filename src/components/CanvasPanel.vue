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
import { useSelectionStore } from '../stores/selection'
import PaperNode from './canvas/PaperNode.vue'
import SuggestPanel from './canvas/SuggestPanel.vue'
import ExportDialog from './canvas/ExportDialog.vue'
import type { PaperIndexEntry, CanvasNode as CNode, CanvasEdge as CEdge, SuggestedEdge, NodePosition } from '../types'

const { t } = useI18n()
const canvasStore = useCanvasStore()
const library = useLibraryStore()
const selectionStore = useSelectionStore()

const emit = defineEmits<{
  'select-paper': [slug: string]
  'close': []
}>()

// ── Vue Flow setup ────────────────────────────────────────────────────────────

const nodeTypes = markRaw({ paper: PaperNode })

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const nodes = ref<any[]>([])
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const edges = ref<any[]>([])

const {
  onConnect,
  addEdges,
  addNodes,
  removeNodes,
  removeEdges,
  onNodeDragStop,
  getViewport,
  setViewport,
  fitView,
} = useVueFlow()

// ── Paper picker dialog ───────────────────────────────────────────────────────

const showPaperPicker = ref(false)
const pickerSearch = ref('')
const pickerError = ref('')

const filteredPapers = computed(() => {
  const q = pickerSearch.value.trim().toLowerCase()
  if (!q) return library.papers
  return library.papers.filter(p =>
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
  paperId: string | null
}>({ show: false, x: 0, y: 0, nodeId: null, edgeId: null, paperId: null })

// Notes pinning in context menu
const ctxNodeSlug = ref<string | null>(null)
const ctxNotes = ref<import('../types').Note[]>([])
const ctxPinnedNotes = ref<string[]>([])
const ctxNotesLoading = ref(false)

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

function buildVfNodes(cnodes: CNode[]): VfNode[] {
  return cnodes.map(cn => {
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
      },
    } satisfies VfNode
  })
}

function buildVfEdges(cedges: CEdge[]): VfEdge[] {
  return cedges.map(ce => ({
    id: ce.edge_id,
    source: ce.from_node_id,
    target: ce.to_node_id,
    label: ce.label,
    markerEnd: MarkerType.ArrowClosed,
    style: { strokeWidth: 1.8 },
    labelStyle: { fontSize: '11px' },
    labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
    type: 'smoothstep',
  } satisfies VfEdge))
}

function extractCanvasNodes(): CNode[] {
  return (nodes.value as VfNode[]).map((n): CNode => ({
    node_id: n.id,
    paper_id: (n.data as Record<string, unknown>).paperId as string,
    x: n.position.x,
    y: n.position.y,
    color: (n.data as Record<string, unknown>).color as string | undefined,
    hover_source: (n.data as Record<string, unknown>).hoverSource as string | undefined,
  }))
}

function extractCanvasEdges(): CEdge[] {
  return (edges.value as VfEdge[]).map((e): CEdge => ({
    edge_id: e.id,
    from_node_id: e.source,
    to_node_id: e.target,
    label: typeof e.label === 'string' ? e.label || undefined : undefined,
  }))
}

// ── Render current canvas from store ─────────────────────────────────────────

async function renderCanvas() {
  const cv = canvasStore.currentCanvas
  if (!cv) return
  nodes.value = buildVfNodes(cv.nodes)
  edges.value = buildVfEdges(cv.edges)
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

onConnect((params: Connection) => {
  if (!params.source || !params.target) return
  const exists = edges.value.some(
    e => e.source === params.source && e.target === params.target
  )
  if (exists) return
  const newEdge: VfEdge = {
    id: `e-${Date.now()}`,
    source: params.source,
    target: params.target,
    markerEnd: MarkerType.ArrowClosed,
    style: { strokeWidth: 1.8 },
    type: 'smoothstep',
    labelStyle: { fontSize: '11px' },
    labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
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

// ── Paper node click → select in right sidebar ────────────────────────────────

function onNodeClick(event: NodeMouseEvent) {
  const paperId = event.node.data?.paperId as string | undefined
  if (paperId) selectPaperById(paperId)
}

// ── Add paper to canvas ───────────────────────────────────────────────────────

function openPaperPicker() {
  pickerSearch.value = ''
  pickerError.value = ''
  showPaperPicker.value = true
}

function addPaperToCanvas(paper: PaperIndexEntry) {
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
    },
  }
  addNodes([newNode])
  showPaperPicker.value = false
  triggerSave()
}

// ── Node hover tooltip ────────────────────────────────────────────────────────

function onNodeMouseEnter(event: NodeMouseEvent) {
  const nd = event.node
  const mouseEvt = event.event as MouseEvent
  hoverPos.value = { x: mouseEvt.clientX, y: mouseEvt.clientY }
  if (hoverTimer) clearTimeout(hoverTimer)
  hoverTimer = setTimeout(async () => {
    hoverNodeId.value = nd.id
    const source = nd.data.hoverSource ?? canvasStore.settings.hover_content_source
    if (source === 'none') { hoverContent.value = ''; return }
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
  return marked(hoverContent.value) as string
})

// ── Context menu ──────────────────────────────────────────────────────────────

async function onNodeContextMenu(event: NodeMouseEvent) {
  event.event.preventDefault()
  const paperId = (event.node.data?.paperId as string) ?? null
  const paper = paperId ? library.papers.find(p => p.id === paperId) : null
  const slug = paper?.slug ?? null

  ctxMenu.value = {
    show: true,
    x: (event.event as MouseEvent).clientX,
    y: (event.event as MouseEvent).clientY,
    nodeId: event.node.id,
    edgeId: null,
    paperId,
  }
  ctxNodeSlug.value = slug
  ctxNotes.value = []
  ctxPinnedNotes.value = []

  if (slug) {
    ctxNotesLoading.value = true
    try {
      const [notes, meta] = await Promise.all([
        invoke<import('../types').Note[]>('list_notes', { slug }),
        invoke<import('../types').PaperMeta>('get_paper_meta', { slug }),
      ])
      ctxNotes.value = notes
      ctxPinnedNotes.value = meta.canvas_notes ?? []
    } catch { /* no notes or meta */ } finally {
      ctxNotesLoading.value = false
    }
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
    paperId: null,
  }
}

function closeCtxMenu() {
  ctxMenu.value = { show: false, x: 0, y: 0, nodeId: null, edgeId: null, paperId: null }
}

function ctxSelectPaper() {
  const paperId = ctxMenu.value.paperId
  closeCtxMenu()
  if (paperId) selectPaperById(paperId)
}

function ctxRemoveNode() {
  const nodeId = ctxMenu.value.nodeId
  if (!nodeId) return
  removeNodes([nodeId])
  removeEdges((edges.value as VfEdge[]).filter(e => e.source === nodeId || e.target === nodeId).map(e => e.id))
  closeCtxMenu()
  triggerSave()
}

async function toggleCtxNotePin(noteId: string) {
  if (!ctxNodeSlug.value) return
  const next = ctxPinnedNotes.value.includes(noteId)
    ? ctxPinnedNotes.value.filter(id => id !== noteId)
    : [...ctxPinnedNotes.value, noteId]
  ctxPinnedNotes.value = next
  await invoke('set_canvas_notes', { slug: ctxNodeSlug.value, noteIds: next })
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

// ── M10: Accept suggestion as real edge ───────────────────────────────────────
function acceptSuggestion(s: SuggestedEdge) {
  const fromNode = (nodes.value as any[]).find(n => n.data?.paperId === s.from_paper_id)
  const toNode = (nodes.value as any[]).find(n => n.data?.paperId === s.to_paper_id)
  if (!fromNode || !toNode) return
  const newEdge = {
    id: `edge-${Date.now()}`,
    source: fromNode.id,
    target: toNode.id,
    type: 'smoothstep',
    markerEnd: MarkerType.ArrowClosed,
    style: { strokeWidth: 1.8 },
    labelStyle: { fontSize: '11px' },
    labelBgStyle: { fill: 'var(--bg-primary)', fillOpacity: 0.9 },
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
  }
}

function onDocClick(e: MouseEvent) {
  if (ctxMenu.value.show) {
    const menu = document.querySelector('.canvas-ctx-menu')
    if (menu && !menu.contains(e.target as Element)) closeCtxMenu()
  }
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

onMounted(async () => {
  if (!canvasStore.canvasList.length) {
    await canvasStore.loadList()
  }
  await canvasStore.loadSettings()
  document.addEventListener('keydown', onKeydown)
  document.addEventListener('pointerdown', onDocClick)
})

onUnmounted(() => {
  document.removeEventListener('keydown', onKeydown)
  document.removeEventListener('pointerdown', onDocClick)
  if (hoverTimer) clearTimeout(hoverTimer)
})

// Refresh nodes when library papers change (validity)
watch(() => library.papers, () => {
  nodes.value = nodes.value.map(n => {
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
      <div v-if="canvasStore.currentCanvas" class="flow-wrap" ref="flowContainerRef">
        <VueFlow
          v-model:nodes="nodes"
          v-model:edges="edges"
          :node-types="nodeTypes"
          :connection-mode="ConnectionMode.Loose"
          :default-edge-options="{ type: 'smoothstep', markerEnd: MarkerType.ArrowClosed }"
          :snap-to-grid="false"
          fit-view-on-init
          class="canvas-flow"
          @node-click="onNodeClick"
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
          <button class="ctx-item" @click="ctxSelectPaper">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
              <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
            </svg>
            {{ t('canvas.viewPaper') }}
          </button>
          <!-- Note pinning section -->
          <div class="ctx-divider" />
          <div class="ctx-section-label">{{ t('canvas.hoverNotes') }}</div>
          <div v-if="ctxNotesLoading" class="ctx-item ctx-item--muted">{{ t('canvas.loading') }}</div>
          <div v-else-if="!ctxNotes.length" class="ctx-item ctx-item--muted">{{ t('canvas.noNotes') }}</div>
          <button
            v-for="note in ctxNotes"
            :key="note.id"
            class="ctx-item ctx-item--note"
            @click.stop="toggleCtxNotePin(note.id)"
          >
            <svg v-if="ctxPinnedNotes.includes(note.id)" width="12" height="12" viewBox="0 0 24 24" fill="var(--accent)" stroke="none">
              <path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/>
            </svg>
            <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="9"/>
            </svg>
            {{ note.title }}
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
        <template v-if="ctxMenu.edgeId">
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
.ctx-item--muted { color: var(--text-tertiary); font-size: 12px; cursor: default; pointer-events: none; }
.ctx-item--muted:hover { background: transparent; }
.ctx-item--note { font-size: 12px; }
.ctx-section-label {
  padding: 4px 14px 2px;
  font-size: 10px;
  font-weight: 700;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.05em;
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
</style>
