<script setup lang="ts">
import { ref, computed, watch, nextTick, onMounted, onBeforeUnmount } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { useI18n } from 'vue-i18n'
import { storeToRefs } from 'pinia'
import { RecycleScroller } from 'vue-virtual-scroller'
import { invoke } from '@tauri-apps/api/core'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { useLibraryStore } from '../stores/library'
import { useSelectionStore } from '../stores/selection'
import { useReaderStore } from '../stores/reader'
import { useCollectionsStore } from '../stores/collections'
import { useImportStore } from '../stores/import'
import { usePaperTasksStore, type AiSummaryJob, type PaperTaskKind } from '../stores/paperTasks'
import { useRagStore } from '../stores/rag'
import { buildChunks } from '../utils/chunker'
import type { PaperVectorizeInput, ChunkInput } from '../types'
import StatusBadges from './StatusBadges.vue'
import CollectionCascadeMenu from './CollectionCascadeMenu.vue'
import { titleInitialCaps } from '../utils/text'
import { noteBadgeStyle } from '../utils/noteBadges'
import type { Collection, Note, PaperIndexEntry, PaperMeta, SortField, SortDir } from '../types'

const { t } = useI18n()
const library = useLibraryStore()
const selection = useSelectionStore()
const reader = useReaderStore()
const collectionsStore = useCollectionsStore()
const importStore = useImportStore()
const paperTasks = usePaperTasksStore()
const { aiSummaryJobs, aiMetaSlug, abstractSlug } = storeToRefs(paperTasks)
const ragStore = useRagStore()
const isCollectionView = computed(() => !!selection.activeCollectionId)

async function pickAndImport() {
  const collectionId = selection.activeCollectionId
  if (!library.currentPath || !collectionId) return
  try {
    const paths = await invoke<string[]>('pick_pdf_files')
    if (paths.length > 0) importStore.importFiles(paths, collectionId)
  } catch (e) { console.error('Import pick failed:', e) }
}

function openInReader(item: PaperIndexEntry) {
  reader.openPaper(item.slug, item.title)
}

function displayTitle(title: string) {
  return titleInitialCaps(title)
}

// ── Sort ──────────────────────────────────────────────────────────────────────
const sortField = ref<SortField | null>('added_at')
const sortDir = ref<SortDir>('desc')

function toggleSort(field: SortField) {
  if (sortField.value === field) {
    sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
  } else {
    sortField.value = field
    sortDir.value = (field === 'added_at' || field === 'cite_count') ? 'desc' : 'asc'
  }
}

function clearSort(field?: SortField) {
  if (!field) return
  if (sortField.value === field) {
    sortField.value = null
  }
}

function compareValue(a: PaperIndexEntry, b: PaperIndexEntry): number {
  switch (sortField.value) {
    case 'title':      return a.title.localeCompare(b.title)
    case 'year':       return (a.year ?? 0) - (b.year ?? 0)
    case 'added_at':   return a.added_at.localeCompare(b.added_at)
    case 'authors':    return (a.authors[0] ?? '').localeCompare(b.authors[0] ?? '')
    case 'venue':      return (a.venue ?? '').localeCompare(b.venue ?? '')
    case 'cite_count': return (a.cite_count ?? -1) - (b.cite_count ?? -1)
    default:           return 0
  }
}

// ── Column configuration ──────────────────────────────────────────────────────
type ColId = 'tags' | 'title' | 'notes' | 'authors' | 'venue' | 'year' | 'added_at' | 'status' | 'source' | 'cite_count'

const COL_META: Record<ColId, {
  id: ColId; labelKey: string; defaultWidth: number; minWidth: number; sortField?: SortField
}> = {
  tags:       { id: 'tags',       labelKey: 'list.tags',      defaultWidth: 160, minWidth: 80 },
  title:      { id: 'title',      labelKey: 'list.title',     defaultWidth: 440, minWidth: 180, sortField: 'title' },
  notes:      { id: 'notes',      labelKey: 'list.notes',     defaultWidth: 150, minWidth: 90 },
  authors:    { id: 'authors',    labelKey: 'list.authors',   defaultWidth: 160, minWidth: 90,  sortField: 'authors' },
  venue:      { id: 'venue',      labelKey: 'list.venue',     defaultWidth: 130, minWidth: 80,  sortField: 'venue' },
  year:       { id: 'year',       labelKey: 'list.year',      defaultWidth: 60,  minWidth: 48,  sortField: 'year' },
  added_at:   { id: 'added_at',   labelKey: 'list.addedAt',   defaultWidth: 90,  minWidth: 78,  sortField: 'added_at' },
  status:     { id: 'status',     labelKey: 'list.status',    defaultWidth: 160, minWidth: 120 },
  source:     { id: 'source',     labelKey: 'list.source',    defaultWidth: 72,  minWidth: 60 },
  cite_count: { id: 'cite_count', labelKey: 'list.citeCount', defaultWidth: 72,  minWidth: 56,  sortField: 'cite_count' },
}
const ALL_COL_IDS: ColId[] = ['tags', 'title', 'notes', 'authors', 'venue', 'year', 'added_at', 'status', 'source', 'cite_count']
const COL_STATE_KEY = 'argus:col-state-v9'
const LEGACY_COL_STATE_KEYS = ['argus:col-state-v8']

// ── Import source helpers ─────────────────────────────────────────────────────
type ImportSource = 'file' | 'arxiv' | 'url'

function normalizeSource(source?: string | null): ImportSource {
  return source === 'arxiv' || source === 'url' || source === 'file' ? source : 'file'
}

function paperSource(item: PaperIndexEntry): ImportSource {
  return normalizeSource(item.import_source)
}

const SOURCE_LABEL: Record<ImportSource, string> = {
  arxiv: 'ArXiv',
  file:  '文件',
  url:   '链接',
}
const SOURCE_BG: Record<ImportSource, string> = {
  arxiv: 'var(--source-arxiv-bg)',
  file:  'var(--source-file-bg)',
  url:   'var(--source-url-bg)',
}
const SOURCE_TEXT: Record<ImportSource, string> = {
  arxiv: 'var(--source-arxiv-text)',
  file:  'var(--source-file-text)',
  url:   'var(--source-url-text)',
}

// ── Persist / restore column state ───────────────────────────────────────────
function loadColState() {
  const defaults = {
    order:   [...ALL_COL_IDS] as ColId[],
    visible: new Set<ColId>(['tags', 'title', 'notes', 'authors', 'venue', 'year', 'status', 'source']),
    widths:  Object.fromEntries(ALL_COL_IDS.map(id => [id, COL_META[id].defaultWidth])) as Record<ColId, number>,
  }
  try {
    let raw = localStorage.getItem(COL_STATE_KEY)
    let fromLegacy = false
    if (!raw) {
      for (const key of LEGACY_COL_STATE_KEYS) {
        raw = localStorage.getItem(key)
        if (raw) {
          fromLegacy = true
          break
        }
      }
    }
    if (!raw) return defaults
    const p = JSON.parse(raw)
    const saved: ColId[] = (p.order ?? []).filter((id: string) => (ALL_COL_IDS as string[]).includes(id))
    if (!saved.includes('notes')) {
      const titleIdx = saved.indexOf('title')
      saved.splice(titleIdx >= 0 ? titleIdx + 1 : 0, 0, 'notes')
    }
    for (const id of ALL_COL_IDS) if (!saved.includes(id)) saved.push(id)
    const visible = new Set<ColId>((p.visible ?? [...defaults.visible]).filter((id: string) => (ALL_COL_IDS as string[]).includes(id)) as ColId[])
    if (fromLegacy) visible.add('notes')
    return {
      order:   saved,
      visible,
      widths:  { ...defaults.widths, ...p.widths },
    }
  } catch { return defaults }
}

const _init = loadColState()
const colOrder    = ref<ColId[]>(_init.order)
const visibleCols = ref<Set<ColId>>(_init.visible)
const colWidths   = ref<Record<ColId, number>>(_init.widths)
const showColPicker = ref(false)
const headerRef = ref<HTMLElement | null>(null)
const headerScrollRef = ref<HTMLElement | null>(null)
const bodyScrollRef = ref<HTMLElement | null>(null)
const bottomScrollRef = ref<HTMLElement | null>(null)

function saveColState() {
  localStorage.setItem(COL_STATE_KEY, JSON.stringify({
    order:   colOrder.value,
    visible: [...visibleCols.value],
    widths:  colWidths.value,
  }))
}

function toggleCol(id: ColId) {
  const s = new Set(visibleCols.value)
  if (s.has(id)) s.delete(id); else s.add(id)
  visibleCols.value = s
  saveColState()
}

const orderedVisibleCols = computed(() =>
  colOrder.value.filter(id => visibleCols.value.has(id)).map(id => COL_META[id])
)

const TRAILING_SPACER = 80 // reserves space so hdr-controls never covers the last column

const gridCols = computed(() => {
  const parts = ['28px']
  for (const col of orderedVisibleCols.value) parts.push(colWidths.value[col.id] + 'px')
  parts.push(`${TRAILING_SPACER}px`)
  return parts.join(' ')
})

const tableWidthPx = computed(() => {
  const width = orderedVisibleCols.value.reduce((sum, col) => sum + colWidths.value[col.id], 28)
  return `${width + TRAILING_SPACER}px`
})

function bodyScrollEl(): HTMLElement | null {
  return bodyScrollRef.value
}

function syncHorizontalScroll(left: number, source?: HTMLElement | null) {
  for (const el of [headerScrollRef.value, bodyScrollRef.value, bottomScrollRef.value]) {
    if (!el || el === source) continue
    if (Math.abs(el.scrollLeft - left) > 0.5) el.scrollLeft = left
  }
}

function syncHorizontalFromEvent(e: Event) {
  const scroller = (e as Event).currentTarget as HTMLElement | null | undefined
  if (!scroller || typeof scroller.scrollLeft !== 'number') return
  syncHorizontalScroll(scroller.scrollLeft, scroller)
}

function onHeaderWheel(e: WheelEvent) {
  const body = bodyScrollEl()
  if (!body) return

  const maxScroll = body.scrollWidth - body.clientWidth
  if (maxScroll <= 0) return

  const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY
  if (delta === 0) return

  const next = Math.max(0, Math.min(maxScroll, body.scrollLeft + delta))
  if (next === body.scrollLeft) return

  e.preventDefault()
  syncHorizontalScroll(next)
}

// ── Column resize (mouse-based) ───────────────────────────────────────────────
let _resizeId: ColId | null = null
let _resizeStartX = 0
let _resizeStartW = 0

function startResize(e: MouseEvent, id: ColId) {
  e.preventDefault()
  e.stopPropagation()
  _resizeId = id
  _resizeStartX = e.clientX
  _resizeStartW = colWidths.value[id]
  document.body.style.cursor = 'col-resize'
  document.body.style.userSelect = 'none'
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', stopResize, { once: true })
}

function onResizeMove(e: MouseEvent) {
  if (!_resizeId) return
  const newW = Math.max(COL_META[_resizeId].minWidth, _resizeStartW + (e.clientX - _resizeStartX))
  colWidths.value = { ...colWidths.value, [_resizeId]: newW }
}

function stopResize() {
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  window.removeEventListener('mousemove', onResizeMove)
  saveColState()
  _resizeId = null
}

// ── Column reorder (pointer-based insertion line) ─────────────────────────────
const dragColId = ref<ColId | null>(null)
const dragInsertIndex = ref<number | null>(null)
const isColumnDragging = computed(() => dragColId.value !== null && dragInsertIndex.value !== null)
let _colDragSourceId: ColId | null = null
let _colDragStartX = 0
let _colDragStartY = 0
let _colDragMoved = false
let _suppressColClickUntil = 0

function getVisibleHeaderRects() {
  if (!headerRef.value) return []
  return Array.from(headerRef.value.querySelectorAll<HTMLElement>('.optional-hdr[data-col-id]'))
    .map(el => ({ id: el.dataset.colId as ColId, rect: el.getBoundingClientRect() }))
}

function getInsertIndexFromX(clientX: number): number {
  const rects = getVisibleHeaderRects()
  for (let i = 0; i < rects.length; i++) {
    if (clientX < rects[i].rect.left + rects[i].rect.width / 2) return i
  }
  return rects.length
}

const dragInsertStyle = computed(() => {
  const header = headerRef.value
  const index = dragInsertIndex.value
  const rects = getVisibleHeaderRects()
  if (!header || index === null || rects.length === 0) return { display: 'none' }

  const headerRect = header.getBoundingClientRect()
  let x = rects[0].rect.left
  if (index <= 0) x = rects[0].rect.left
  else if (index >= rects.length) x = rects[rects.length - 1].rect.right
  else x = rects[index].rect.left

  return { left: `${x - headerRect.left}px` }
})

function commitColumnReorder(fromId: ColId, rawInsertIndex: number) {
  const visibleOrder = colOrder.value.filter(id => visibleCols.value.has(id))
  const fromIndex = visibleOrder.indexOf(fromId)
  if (fromIndex === -1) return

  let insertIndex = Math.max(0, Math.min(rawInsertIndex, visibleOrder.length))
  if (fromIndex < insertIndex) insertIndex -= 1
  if (fromIndex === insertIndex) return

  visibleOrder.splice(fromIndex, 1)
  visibleOrder.splice(insertIndex, 0, fromId)

  const nextVisible = [...visibleOrder]
  colOrder.value = colOrder.value.map(id => visibleCols.value.has(id) ? nextVisible.shift()! : id)
  saveColState()
}

function onColPointerDown(e: PointerEvent, id: ColId) {
  if (e.button !== 0) return
  if ((e.target as HTMLElement).closest('.resize-handle')) return

  _colDragSourceId = id
  _colDragStartX = e.clientX
  _colDragStartY = e.clientY
  _colDragMoved = false

  window.addEventListener('pointermove', onColPointerMove)
  window.addEventListener('pointerup', onColPointerUp, { once: true })
  window.addEventListener('pointercancel', onColPointerCancel, { once: true })
}

function onColPointerMove(e: PointerEvent) {
  if (!_colDragSourceId) return

  const dx = e.clientX - _colDragStartX
  const dy = e.clientY - _colDragStartY
  if (!_colDragMoved && Math.hypot(dx, dy) < 5) return

  if (!_colDragMoved) {
    _colDragMoved = true
    dragColId.value = _colDragSourceId
    document.body.style.cursor = 'grabbing'
    document.body.style.userSelect = 'none'
  }

  dragInsertIndex.value = getInsertIndexFromX(e.clientX)
}

function onColPointerUp() {
  if (_colDragMoved && dragColId.value && dragInsertIndex.value !== null) {
    commitColumnReorder(dragColId.value, dragInsertIndex.value)
  }
  cleanupColumnDrag()
}

function onColPointerCancel() {
  cleanupColumnDrag()
}

function cleanupColumnDrag() {
  if (_colDragMoved) {
    _suppressColClickUntil = Date.now() + 250
  }
  _colDragSourceId = null
  _colDragMoved = false
  dragColId.value = null
  dragInsertIndex.value = null
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  window.removeEventListener('pointermove', onColPointerMove)
  window.removeEventListener('pointerup', onColPointerUp)
  window.removeEventListener('pointercancel', onColPointerCancel)
}

function onColClick(col: { sortField?: SortField }) {
  if (Date.now() < _suppressColClickUntil) return
  if (col.sortField) toggleSort(col.sortField)
}

function onColContextMenu(col: { sortField?: SortField }) {
  clearSort(col.sortField)
}

function onGlobalPointerDown(e: PointerEvent) {
  if (!ctxMenu.value) return
  const target = e.target as HTMLElement | null
  if (target?.closest('.ctx-menu')) return
  closeCtx()
}

function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') closeCtx()
}

function onCanvasNotesUpdated(e: Event) {
  const detail = (e as CustomEvent<{ slug?: string }>).detail
  if (detail?.slug) refreshCanvasNoteTitle(detail.slug)
}

onMounted(() => {
  document.addEventListener('pointerdown', onGlobalPointerDown, true)
  document.addEventListener('keydown', onGlobalKeydown)
  window.addEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  paperTasks.startListening()
})

onBeforeUnmount(() => {
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', stopResize)
  document.removeEventListener('pointerdown', onGlobalPointerDown, true)
  document.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('argus-canvas-notes-updated', onCanvasNotesUpdated)
  cleanupColumnDrag()
  for (const slug of aiSummaryStreamUnlisteners.keys()) cleanupAiSummaryStream(slug)
})

// ── Collection papers ─────────────────────────────────────────────────────────
const collectionPapers = ref<PaperIndexEntry[]>([])

let collectionLoadSeq = 0

async function refreshCollectionPapers(id: string | null) {
  const seq = ++collectionLoadSeq
  if (!id) { collectionPapers.value = []; return }
  // Show the whole subtree for every collection so the list count matches the
  // recursive badge shown in the sidebar (paperCountsByCollection). Both use the
  // same childMap + existingPaperIds filtering, so the two stay in sync.
  const papers = collectionsStore.listAllPapersInTree(id)
  // Guard against a stale response overwriting a newer selection (parity with
  // the canvasNoteLoadSeq pattern below); harmless while sync, safe if this
  // ever becomes async again.
  if (seq !== collectionLoadSeq) return
  collectionPapers.value = papers
}

watch(() => selection.activeCollectionId, (id) => refreshCollectionPapers(id ?? null), { immediate: true })

watch(() => collectionsStore.file.assignments, () => {
  refreshCollectionPapers(selection.activeCollectionId ?? null)
}, { deep: true })

// After a library refresh (e.g. paper rename during import), re-sync collection papers
// so stale slugs (tempSlug → finalSlug) don't break the right-panel meta load.
watch(() => library.papers, () => {
  refreshCollectionPapers(selection.activeCollectionId ?? null)
})

// ── Filtered / sorted list ────────────────────────────────────────────────────
const filtered = computed<PaperIndexEntry[]>(() => {
  const nav = selection.activeNav
  if (nav === 'inbox' || nav === 'search') return []
  let list = nav.startsWith('collection:') ? collectionPapers.value : library.papers
  const activeTag = selection.tagFilter ?? (nav.startsWith('tag:') ? nav.slice(4) : null)
  if (activeTag) list = list.filter(p => p.tags?.includes(activeTag))
  return list
})

const sorted = computed(() => {
  if (!sortField.value) return [...filtered.value]
  return [...filtered.value].sort((a, b) => {
    const cmp = compareValue(a, b)
    return sortDir.value === 'asc' ? cmp : -cmp
  })
})

const canvasNoteTitles = ref<Record<string, string[]>>({})
let canvasNoteLoadSeq = 0

function canvasNotesFor(item: PaperIndexEntry): string[] {
  return canvasNoteTitles.value[item.slug] ?? []
}

async function refreshCanvasNoteTitles(slugs: string[]) {
  const unique = Array.from(new Set(slugs.filter(Boolean)))
  if (unique.length === 0) {
    canvasNoteTitles.value = {}
    return
  }

  const seq = ++canvasNoteLoadSeq
  try {
    const map = await invoke<Record<string, string[]>>('get_canvas_note_titles_map', { slugs: unique })
    if (seq === canvasNoteLoadSeq) canvasNoteTitles.value = map
  } catch {
    if (seq === canvasNoteLoadSeq) canvasNoteTitles.value = {}
  }
}

async function refreshCanvasNoteTitle(slug: string) {
  if (!slug) return
  try {
    const map = await invoke<Record<string, string[]>>('get_canvas_note_titles_map', { slugs: [slug] })
    canvasNoteTitles.value = {
      ...canvasNoteTitles.value,
      [slug]: map[slug] ?? [],
    }
  } catch {
    canvasNoteTitles.value = {
      ...canvasNoteTitles.value,
      [slug]: [],
    }
  }
}

watch(
  () => sorted.value.map(p => p.slug).join('|'),
  () => refreshCanvasNoteTitles(sorted.value.map(p => p.slug)),
  { immediate: true },
)

function formatAuthors(authors: string[]): string {
  if (authors.length === 0) return '—'
  if (authors.length <= 2) return authors.join(', ')
  return authors[0] + ' et al.'
}

function formatDate(iso: string): string {
  const d = new Date(iso)
  return `${d.getFullYear()}/${String(d.getMonth() + 1).padStart(2, '0')}/${String(d.getDate()).padStart(2, '0')}`
}

function clickTag(e: MouseEvent, tag: string) {
  e.stopPropagation()
  selection.toggleTagFilter(tag)
}

async function confirmRemoveTag(item: PaperIndexEntry, tag: string) {
  if (!confirm(`确定要删除标签 "${tag}" 吗？`)) return
  const tags = (item.tags ?? []).filter(t => t !== tag)
  try {
    await savePaperTags(item, tags)
  } catch (e) {
    showError(String(e))
  }
}

// ── Paper row drag (pointer-based, no HTML5 drag API → no macOS green "+") ────
const dragGhostItem = ref<PaperIndexEntry | null>(null)
const dragGhostPos  = ref({ x: 0, y: 0 })

function onRowMouseDown(e: MouseEvent, item: PaperIndexEntry) {
  if (e.button !== 0) return
  const startX = e.clientX, startY = e.clientY
  let dragging = false
  let hoverCollId: string | null = null

  function findCollId(x: number, y: number): string | null {
    for (const el of document.elementsFromPoint(x, y)) {
      const id = (el as HTMLElement).dataset?.collectionId
      if (id) return id
    }
    return null
  }

  function effectiveDragTarget(x: number, y: number): string | null {
    const id = findCollId(x, y)
    if (!id || collectionsStore.isTopLevel(id)) return null
    return id
  }

  function onMove(e: MouseEvent) {
    if (!dragging && Math.hypot(e.clientX - startX, e.clientY - startY) < 6) return
    if (!dragging) {
      dragging = true
      dragGhostItem.value = item
      document.body.style.cursor = 'grabbing'
    }
    dragGhostPos.value = { x: e.clientX + 16, y: e.clientY + 10 }
    const collId = effectiveDragTarget(e.clientX, e.clientY)
    if (collId !== hoverCollId) {
      hoverCollId = collId
      document.dispatchEvent(new CustomEvent('argus-paper-drag-over', { detail: { collectionId: collId } }))
    }
  }

  async function onUp(e: MouseEvent) {
    window.removeEventListener('mousemove', onMove)
    window.removeEventListener('mouseup', onUp)
    document.body.style.cursor = ''
    if (!dragging) return
    dragGhostItem.value = null
    document.dispatchEvent(new CustomEvent('argus-paper-drag-over', { detail: { collectionId: null } }))
    const collId = effectiveDragTarget(e.clientX, e.clientY)
    if (collId) await collectionsStore.movePaper(item.id, collId)
  }

  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}

// ── Context menu ──────────────────────────────────────────────────────────────
const CONTEXT_MENU_MARGIN = 10
const ctxMenuEl = ref<HTMLElement | null>(null)
type ContextMenuState = {
  x: number
  y: number
  item: PaperIndexEntry
  showColls: boolean
  tagInput: string
  constrained: boolean
}
const ctxMenu = ref<ContextMenuState | null>(null)
const contextMenuRootCollections = computed(() => {
  const seen = new Set<string>()

  function mark(collections: Collection[]) {
    for (const col of collections) {
      if (seen.has(col.id)) continue
      seen.add(col.id)
      mark(collectionsStore.childrenOf(col.id))
    }
  }

  const roots = collectionsStore.topLevel.slice()
  mark(roots)

  const orphanRoots: Collection[] = []
  for (const col of collectionsStore.file.collections
    .filter(col => !seen.has(col.id))
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }))) {
    if (seen.has(col.id)) continue
    orphanRoots.push(col)
    mark([col])
  }

  return [...roots, ...orphanRoots]
})

async function positionContextMenu() {
  await nextTick()
  if (!ctxMenu.value || !ctxMenuEl.value) return

  const rect = ctxMenuEl.value.getBoundingClientRect()
  const viewportW = window.innerWidth
  const viewportH = window.innerHeight
  const constrained = rect.height > viewportH - CONTEXT_MENU_MARGIN * 2
  const boundedHeight = constrained ? viewportH - CONTEXT_MENU_MARGIN * 2 : rect.height
  const maxX = Math.max(CONTEXT_MENU_MARGIN, viewportW - rect.width - CONTEXT_MENU_MARGIN)
  const maxY = Math.max(CONTEXT_MENU_MARGIN, viewportH - boundedHeight - CONTEXT_MENU_MARGIN)
  const x = Math.min(Math.max(CONTEXT_MENU_MARGIN, ctxMenu.value.x), maxX)
  const y = Math.min(Math.max(CONTEXT_MENU_MARGIN, ctxMenu.value.y), maxY)

  if (x !== ctxMenu.value.x || y !== ctxMenu.value.y || constrained !== ctxMenu.value.constrained) {
    ctxMenu.value = { ...ctxMenu.value, x, y, constrained }
  }
}

async function openCtx(e: MouseEvent, item: PaperIndexEntry) {
  e.preventDefault()
  ctxMenu.value = { x: e.clientX, y: e.clientY, item, showColls: false, tagInput: '', constrained: false }
  await positionContextMenu()
}
function closeCtx() { ctxMenu.value = null }

const suggestedContextTags = computed(() => {
  if (!ctxMenu.value) return []
  const owned = new Set(ctxMenu.value.item.tags ?? [])
  const query = ctxMenu.value.tagInput.trim().toLowerCase()
  return library.allTags
    .filter(tag => tag.trim())
    .filter(tag => !owned.has(tag))
    .filter(tag => !CATEGORY_TAGS.has(tag))
    .filter(tag => !query || tag.toLowerCase().includes(query))
})

async function addToCollection(collectionId: string) {
  if (!ctxMenu.value) return
  await collectionsStore.addPaper(ctxMenu.value.item.id, collectionId)
  closeCtx()
}

async function removeFromCurrentCollection() {
  if (!ctxMenu.value || !selection.activeCollectionId) return
  await collectionsStore.removePaper(ctxMenu.value.item.id, selection.activeCollectionId)
  await refreshCollectionPapers(selection.activeCollectionId)
  closeCtx()
}

async function savePaperTags(item: PaperIndexEntry, tags: string[]) {
  const meta = await invoke<PaperMeta>('get_paper_meta', { slug: item.slug })
  meta.tags = tags
  await invoke('save_paper_meta', { slug: item.slug, meta })
  item.tags = [...tags]
  window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: item.slug, meta } }))
  await library.refresh()
}

async function addTagFromContext() {
  if (!ctxMenu.value) return
  const raw = ctxMenu.value.tagInput.trim()
  if (!raw) return

  const additions = raw
    .split(/[,，]/)
    .map(tag => tag.trim())
    .filter(Boolean)
  if (additions.length === 0) return

  const tags = [...(ctxMenu.value.item.tags ?? [])]
  for (const tag of additions) {
    if (!tags.includes(tag)) tags.push(tag)
  }
  ctxMenu.value.tagInput = ''

  try {
    await savePaperTags(ctxMenu.value.item, tags)
  } catch (e) {
    showError(String(e))
  }
}

async function addSuggestedTag(tag: string) {
  if (!ctxMenu.value) return
  const tags = [...(ctxMenu.value.item.tags ?? [])]
  if (tags.includes(tag)) return
  tags.push(tag)
  ctxMenu.value.tagInput = ''

  try {
    await savePaperTags(ctxMenu.value.item, tags)
  } catch (e) {
    showError(String(e))
  }
}

async function removeTagFromContext(tag: string) {
  if (!ctxMenu.value) return
  if (!confirm(`确定要删除标签 "${tag}" 吗？`)) return
  const tags = (ctxMenu.value.item.tags ?? []).filter(t => t !== tag)
  try {
    await savePaperTags(ctxMenu.value.item, tags)
  } catch (e) {
    showError(String(e))
  }
}

const STATUS_LABELS: Record<string, string> = { unread: '未读', reading: '阅读中', read: '已读' }
const STATUS_COLORS: Record<string, string> = {
  unread: 'var(--text-tertiary)', reading: '#f59e0b', read: '#22c55e',
}

const PAPER_CATEGORIES = [
  {
    tag: '理论',
    icon: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/></svg>',
    color: '#7c3aed',
    bg: '#f3e8ff',
  },
  {
    tag: '方法',
    icon: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>',
    color: '#2563eb',
    bg: '#dbeafe',
  },
  {
    tag: '数据集',
    icon: '<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/></svg>',
    color: '#059669',
    bg: '#d1fae5',
  },
]

function categoryIconFor(tag: string): string | undefined {
  return PAPER_CATEGORIES.find(c => c.tag === tag)?.icon
}

function categoryStyleFor(tag: string): { color: string; background: string } | undefined {
  const cat = PAPER_CATEGORIES.find(c => c.tag === tag)
  return cat ? { color: cat.color, background: cat.bg } : undefined
}

const CATEGORY_TAGS = new Set(PAPER_CATEGORIES.map(c => c.tag))

async function toggleCategoryTag(item: PaperIndexEntry, tag: string) {
  const tags = [...(item.tags ?? [])]
  const idx = tags.indexOf(tag)
  if (idx >= 0) tags.splice(idx, 1)
  else tags.push(tag)
  try {
    await savePaperTags(item, tags)
  } catch (e) {
    showError(String(e))
  }
}

async function setReadingStatus(item: PaperIndexEntry, status: string) {
  closeCtx()
  try {
    await invoke('set_reading_status', { slug: item.slug, status })
    item.reading_status = status
    await library.refresh()
  } catch (e: unknown) { console.error('set_reading_status failed:', e) }
}

async function deletePaper(item: PaperIndexEntry) {
  closeCtx()
  if (!confirm(`${t('paper.deleteConfirm').replace('{title}', item.title)}`)) return
  try {
    await invoke('delete_paper', { slug: item.slug })
    library.removePaper(item.slug)
    collectionsStore.file.assignments = collectionsStore.file.assignments.filter(a => a.paper_id !== item.id)
    collectionPapers.value = collectionPapers.value.filter(p => p.slug !== item.slug)
    if (selection.selectedSlug === item.slug) selection.selectPaper('')
    if (reader.openSlug === item.slug) reader.closePaper()
  } catch (e: unknown) { showError(String(e)) }
}

async function openPaperInFinder(item: PaperIndexEntry) {
  closeCtx()
  try {
    await invoke('open_paper_folder', { slug: item.slug })
  } catch (e: unknown) {
    showError(String(e))
  }
}

async function copyPaperPdf(item: PaperIndexEntry) {
  closeCtx()
  try {
    await invoke('copy_paper_pdf', { slug: item.slug })
  } catch (e: unknown) {
    showError(String(e))
  }
}

// ── Inline error notification ─────────────────────────────────────────────────
const actionError = ref<string | null>(null)
let errorTimer: ReturnType<typeof setTimeout> | null = null

function showError(msg: string) {
  if (errorTimer) clearTimeout(errorTimer)
  actionError.value = msg
  errorTimer = setTimeout(() => { actionError.value = null }, 6000)
}

// ── Export literature list as PDF ─────────────────────────────────────────────
const exportBusy = ref(false)
const exportDone = ref(false)
let exportDoneTimer: ReturnType<typeof setTimeout> | null = null

async function exportToPdf() {
  if (exportBusy.value) return
  const papers = sorted.value
  if (papers.length === 0) return

  const savePath = await dialogSave({
    title: t('list.exportList'),
    defaultPath: 'literature-list.pdf',
    filters: [{ name: 'PDF', extensions: ['pdf'] }],
  })
  if (!savePath) return

  exportBusy.value = true
  try {
    // jspdf is export-only — load it on demand instead of at startup.
    const { jsPDF } = await import('jspdf')
    const doc = new jsPDF({ orientation: 'landscape', unit: 'pt', format: 'a4' })
    const pageW = doc.internal.pageSize.getWidth()
    const pageH = doc.internal.pageSize.getHeight()
    const margin = 36
    const contentW = pageW - margin * 2

    // Title
    doc.setFont('helvetica', 'bold')
    doc.setFontSize(16)
    doc.text('Literature List', margin, margin + 12)
    doc.setFont('helvetica', 'normal')
    doc.setFontSize(9)
    doc.setTextColor(130)
    const d = new Date()
    const dateStr = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
    doc.text(`${papers.length} papers  ${dateStr}`, margin, margin + 28)
    doc.setTextColor(0)

    // Column layout
    const colW = {
      num:     24,
      title:   contentW * 0.46,
      authors: contentW * 0.22,
      venue:   contentW * 0.22,
      year:    contentW * 0.10 - 24,
    }
    const colX = {
      num:     margin,
      title:   margin + 24,
      authors: margin + 24 + colW.title,
      venue:   margin + 24 + colW.title + colW.authors,
      year:    margin + 24 + colW.title + colW.authors + colW.venue,
    }
    const lineH = 11   // line height in pt for font-size 8
    const cellPadV = 6 // top/bottom padding inside each row
    let y = margin + 50

    // Header row
    doc.setFillColor(238, 238, 244)
    doc.rect(margin, y, contentW, lineH + cellPadV * 2, 'F')
    doc.setFont('helvetica', 'bold')
    doc.setFontSize(8)
    doc.setTextColor(90)
    const hdrY = y + cellPadV + lineH - 2
    doc.text('#',       colX.num + 2,     hdrY)
    doc.text('Title',   colX.title + 2,   hdrY)
    doc.text('Authors', colX.authors + 2, hdrY)
    doc.text('Venue',   colX.venue + 2,   hdrY)
    doc.text('Year',    colX.year + 2,    hdrY)
    y += lineH + cellPadV * 2

    // Data rows
    doc.setFont('helvetica', 'normal')
    doc.setFontSize(8)

    for (let i = 0; i < papers.length; i++) {
      const p = papers[i]
      const authors = p.authors.length === 0 ? '-'
        : p.authors.length <= 2 ? p.authors.join(', ')
        : p.authors[0] + ' et al.'

      const titleLines  = doc.splitTextToSize(p.title, colW.title - 4) as string[]
      const authorLines = doc.splitTextToSize(authors, colW.authors - 4) as string[]
      const venueLines  = doc.splitTextToSize(p.venue || '-', colW.venue - 4) as string[]

      const rowH = Math.max(titleLines.length, authorLines.length, venueLines.length) * lineH + cellPadV * 2

      if (y + rowH > pageH - margin) {
        doc.addPage()
        y = margin
      }

      if (i % 2 === 1) {
        doc.setFillColor(249, 249, 252)
        doc.rect(margin, y, contentW, rowH, 'F')
      }

      const textY = y + cellPadV + lineH - 2

      doc.setTextColor(150)
      doc.text(String(i + 1), colX.num + 2, textY)

      doc.setTextColor(20)
      doc.text(titleLines, colX.title + 2, textY, { lineHeightFactor: lineH / 8 })

      doc.setTextColor(80)
      doc.text(authorLines, colX.authors + 2, textY, { lineHeightFactor: lineH / 8 })
      doc.text(venueLines,  colX.venue + 2,   textY, { lineHeightFactor: lineH / 8 })

      doc.setTextColor(120)
      doc.text(String(p.year ?? '-'), colX.year + 2, textY)

      y += rowH
    }

    const pdfBytes = doc.output('arraybuffer')
    await invoke('write_bytes_to_file', { path: savePath, bytes: Array.from(new Uint8Array(pdfBytes)) })

    if (exportDoneTimer) clearTimeout(exportDoneTimer)
    exportDone.value = true
    exportDoneTimer = setTimeout(() => { exportDone.value = false }, 2200)
  } catch (e) {
    showError(String(e))
  } finally {
    exportBusy.value = false
  }
}

const aiSummaryStreamUnlisteners = new Map<string, () => void>()

function setAiSummaryJob(slug: string, patch: Partial<AiSummaryJob>) {
  paperTasks.setAiSummaryJob(slug, patch)
}

function removeAiSummaryJob(slug: string) {
  paperTasks.removeAiSummaryJob(slug)
}

function isAiSummaryActive(slug: string) {
  return paperTasks.isAiSummaryActive(slug)
}

function isAiSummaryDone(item: PaperIndexEntry) {
  return !!item.status.ai_summary_done
}

function isAiSummaryButtonDisabled(item: PaperIndexEntry) {
  return isAiSummaryActive(item.slug) || isAiSummaryDone(item)
}

function cleanupAiSummaryStream(slug: string) {
  aiSummaryStreamUnlisteners.get(slug)?.()
  aiSummaryStreamUnlisteners.delete(slug)
}

async function extractMetaAi(item: PaperIndexEntry) {
  closeCtx()
  paperTasks.setAiMetaTask(item.slug)

  // Switch right sidebar to AI tab so the user sees the streaming response
  window.dispatchEvent(new CustomEvent('argus-switch-sidebar-tab', { detail: { tab: 'ai' } }))

  try {
    const updated = await invoke<PaperMeta>('extract_metadata_ai', { slug: item.slug })
    item.title = updated.title
    item.authors = updated.authors
    item.year = updated.year
    item.venue = updated.venue
    item.status.metadata_fetched = true
    item.status.text_extracted = true
    window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: item.slug, meta: updated } }))
    await library.refresh()
  } catch (e: unknown) {
    showError(String(e))
  } finally {
    paperTasks.clearAiMetaTask()
  }
}

async function extractAbstractAi(item: PaperIndexEntry) {
  closeCtx()
  if (abstractSlug.value === item.slug) return
  paperTasks.setAbstractTask(item.slug)
  try {
    const updatedMeta = await invoke<PaperMeta>('extract_abstract_ai', { slug: item.slug, providerId: null, modelId: null })
    paperTasks.setAbstractTask(null)
    window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: item.slug, meta: updatedMeta } }))
  } catch (e: unknown) { showError(String(e)) }
  finally { paperTasks.setAbstractTask(null) }
}

const vectorizingSlug = ref<string | null>(null)

async function vectorizePaper(item: PaperIndexEntry) {
  if (vectorizingSlug.value) return
  closeCtx()
  vectorizingSlug.value = item.slug
  try {
    await ragStore.load()
    const s = ragStore.settings
    const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: item.slug })
    const chunks: ChunkInput[] = await buildChunks(input, s.chunk_size ?? 512, s.chunk_overlap ?? 50)
    if (chunks.length === 0) return
    await invoke('embed_and_store_chunks', {
      slug: item.slug,
      paperId: input.paper_id,
      paperTitle: input.paper_title,
      chunks,
    })
    item.status.vectorized = true
  } catch (e: unknown) {
    showError(String(e))
  } finally {
    vectorizingSlug.value = null
  }
}

async function ensureFulltextReady(item: PaperIndexEntry, progressKind?: PaperTaskKind) {
  const showProgress = !!progressKind

  if (showProgress) setAiSummaryJob(item.slug, { kind: progressKind, stage: 'text' })
  await invoke('extract_fulltext', { slug: item.slug })

  if (showProgress) setAiSummaryJob(item.slug, { kind: progressKind, stage: 'saving' })
  await invoke('index_paper_search', { slug: item.slug })
  window.dispatchEvent(new CustomEvent('argus-paper-fulltext-updated', { detail: { slug: item.slug } }))
  if (showProgress) {
    const text = await invoke<string>('get_fulltext', { slug: item.slug })
    setAiSummaryJob(item.slug, {
      kind: progressKind,
      stage: 'fulltext_ready',
      fulltextChars: text.length,
    })
  }
}

async function generateAiSummary(item: PaperIndexEntry) {
  closeCtx()
  if (isAiSummaryButtonDisabled(item)) return
  setAiSummaryJob(item.slug, { kind: 'summary', stage: 'queued', generatedChars: 0, message: undefined })
  cleanupAiSummaryStream(item.slug)
  try {
    const eventSafeSlug = item.slug.replace(/[^A-Za-z0-9:_/-]/g, '-')
    const unlistenStream = await listen<{ delta?: string; done?: boolean }>(`ai-summary-${eventSafeSlug}`, (ev) => {
      const job = aiSummaryJobs.value[item.slug]
      if (!job) return
      if (ev.payload.delta) {
        setAiSummaryJob(item.slug, {
          stage: 'ai',
          generatedChars: job.generatedChars + ev.payload.delta.length,
        })
      }
      if (ev.payload.done && job.stage === 'ai') {
        setAiSummaryJob(item.slug, { stage: 'saving' })
      }
    })
    aiSummaryStreamUnlisteners.set(item.slug, unlistenStream)
    await ensureFulltextReady(item, 'summary')
    await invoke<Note>('generate_summary', {
      slug: item.slug,
      providerId: null,
      modelId: null,
    })
    setAiSummaryJob(item.slug, { stage: 'done' })
    window.dispatchEvent(new CustomEvent('argus-notes-updated', { detail: { slug: item.slug } }))
    await library.refresh()
  } catch (e: unknown) {
    setAiSummaryJob(item.slug, { stage: 'error', message: String(e) })
    showError(String(e))
  } finally {
    cleanupAiSummaryStream(item.slug)
    const finalStage = aiSummaryJobs.value[item.slug]?.stage
    const delay = finalStage === 'error' ? 5000 : 1800
    setTimeout(() => {
      const stage = aiSummaryJobs.value[item.slug]?.stage
      if (stage === 'done' || stage === 'error') removeAiSummaryJob(item.slug)
    }, delay)
  }
}

async function reExtract(item: PaperIndexEntry) {
  closeCtx()
  if (isAiSummaryActive(item.slug)) return
  setAiSummaryJob(item.slug, { kind: 'extract', stage: 'queued', generatedChars: 0, message: undefined })
  try {
    await ensureFulltextReady(item, 'extract')
    await library.refresh()
    setAiSummaryJob(item.slug, { kind: 'extract', stage: 'done' })
  } catch (e: unknown) {
    setAiSummaryJob(item.slug, { kind: 'extract', stage: 'error', message: String(e) })
    showError(String(e))
  } finally {
    const delay = aiSummaryJobs.value[item.slug]?.stage === 'error' ? 5000 : 1800
    setTimeout(() => {
      const job = aiSummaryJobs.value[item.slug]
      if (job?.kind === 'extract' && (job.stage === 'done' || job.stage === 'error')) {
        removeAiSummaryJob(item.slug)
      }
    }, delay)
  }
}
</script>

<template>
  <div class="paper-list" @click="closeCtx(); showColPicker = false">

    <!-- ── Action error toast ─────────────────────────────────────────────── -->
    <Transition name="err-toast">
      <div v-if="actionError" class="action-error-toast" @click.self="actionError = null">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="flex-shrink:0">
          <circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>
        </svg>
        <span>{{ actionError }}</span>
      </div>
    </Transition>

    <!-- ── Column header bar ─────────────────────────────────────────────── -->
    <div
      v-if="selection.activeNav !== 'search' && selection.activeNav !== 'inbox'"
      class="header-shell"
    >
      <div
        ref="headerScrollRef"
        class="header-scroll"
        @scroll.passive="syncHorizontalFromEvent"
        @wheel="onHeaderWheel"
      >
        <div
          ref="headerRef"
          class="list-header"
          :class="{ 'is-reordering': isColumnDragging }"
        >
          <!-- Reading status dot column -->
          <div class="hdr-spc" />

          <!-- Data columns: same behavior for title, authors, year, etc. -->
          <div
            v-for="col in orderedVisibleCols"
            :key="col.id"
            :data-col-id="col.id"
            class="hdr-col optional-hdr"
            :class="{
              active:        col.sortField === sortField,
              dragging:      dragColId === col.id,
            }"
            @pointerdown="onColPointerDown($event, col.id)"
            @click.stop="onColClick(col)"
            @contextmenu.prevent.stop="onColContextMenu(col)"
          >
            <span class="hdr-label">{{ t(col.labelKey) }}</span>
            <span v-if="col.sortField && sortField === col.sortField" class="sort-arrow">
              {{ sortDir === 'asc' ? '↑' : '↓' }}
            </span>
            <div
              class="resize-handle"
              @mousedown.stop.prevent="startResize($event, col.id)"
              @click.stop
            />
          </div>

          <div class="hdr-trail" />

          <div
            v-if="isColumnDragging"
            class="col-insert-line"
            :style="dragInsertStyle"
          />
        </div>
      </div>

      <!-- Count + column picker (fixed on the right) -->
      <div class="hdr-controls">
        <span class="list-count">{{ sorted.length }}</span>
        <button
          class="col-picker-btn"
          :class="{ active: exportDone }"
          :disabled="exportBusy"
          :title="exportDone ? t('list.exportListDone') : t('list.exportList')"
          @click.stop="exportToPdf"
        >
          <svg v-if="exportBusy" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="spin">
            <path d="M12 2v4M12 18v4M4.93 4.93l2.83 2.83M16.24 16.24l2.83 2.83M2 12h4M18 12h4M4.93 19.07l2.83-2.83M16.24 7.76l2.83-2.83"/>
          </svg>
          <svg v-else-if="exportDone" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <polyline points="20 6 9 17 4 12"/>
          </svg>
          <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8"/>
            <polyline points="16 6 12 2 8 6"/>
            <line x1="12" y1="2" x2="12" y2="15"/>
          </svg>
        </button>
        <button
          class="col-picker-btn"
          :class="{ active: showColPicker }"
          :title="t('list.colPicker')"
          @click.stop="showColPicker = !showColPicker"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 6h8"/>
            <path d="M16 6h4"/>
            <circle cx="14" cy="6" r="2"/>
            <path d="M4 12h3"/>
            <path d="M11 12h9"/>
            <circle cx="9" cy="12" r="2"/>
            <path d="M4 18h10"/>
            <path d="M18 18h2"/>
            <circle cx="16" cy="18" r="2"/>
          </svg>
        </button>
        <div v-if="showColPicker" class="col-picker-menu" @click.stop>
          <div class="col-picker-title">{{ t('list.colPicker') }}</div>
          <button v-for="id in ALL_COL_IDS" :key="id" class="col-picker-item" @click="toggleCol(id)">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <polyline v-if="visibleCols.has(id)" points="20 6 9 17 4 12"/>
            </svg>
            {{ t(COL_META[id].labelKey) }}
          </button>
        </div>
      </div>
    </div>

    <!-- ── Search results ────────────────────────────────────────────────── -->
    <template v-if="selection.activeNav === 'search'">
      <div class="search-header">
        <span>{{ t('search.results') }}</span>
        <span class="search-count">{{ t('search.hits').replace('{n}', String(selection.searchResults.length)) }}</span>
      </div>
      <div v-if="selection.searchResults.length === 0" class="empty-state">
        <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
        </svg>
        <p>{{ t('search.noResults') }}</p>
      </div>
      <div v-else class="search-results">
        <div
          v-for="hit in selection.searchResults"
          :key="hit.slug"
          class="search-hit"
          :class="{ selected: hit.slug === selection.selectedSlug }"
          @click="selection.selectPaper(hit.slug)"
          @dblclick="reader.openPaper(hit.slug, hit.title)"
        >
          <div class="hit-title" :title="displayTitle(hit.title)">{{ displayTitle(hit.title) }}</div>
          <div class="hit-authors">{{ formatAuthors(hit.authors) }}</div>
          <div class="hit-snippet" v-html="hit.snippet" />
        </div>
      </div>
    </template>

    <!-- ── Inbox placeholder ─────────────────────────────────────────────── -->
    <div v-else-if="selection.activeNav === 'inbox'" class="empty-state">
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/>
        <path d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>
      </svg>
      <p>{{ t('list.inboxEmpty') }}</p>
      <span>{{ t('list.inboxHint') }}</span>
    </div>

    <!-- ── Empty state ───────────────────────────────────────────────────── -->
    <div v-else-if="sorted.length === 0 && !library.isLoading" class="empty-state">
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
        <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
      </svg>
      <p>{{ selection.activeNav.startsWith('collection:') ? t('collections.empty') : t('list.noPapers') }}</p>
      <span>{{ isCollectionView ? t('import.collectionHint') : t('import.selectCollectionHint') }}</span>
      <button
        class="import-btn"
        :disabled="!isCollectionView"
        :title="isCollectionView ? t('import.btnTitle') : t('import.selectCollectionTitle')"
        @click="pickAndImport"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
          <polyline points="14 2 14 8 20 8"/>
          <line x1="12" y1="18" x2="12" y2="12"/>
          <polyline points="9 15 12 18 15 15"/>
        </svg>
        {{ t('import.btn') }}
      </button>
    </div>

    <!-- ── Virtual list ──────────────────────────────────────────────────── -->
    <div
      v-else
      class="body-shell"
    >
      <div
        ref="bodyScrollRef"
        class="body-scroll"
        @scroll.passive="syncHorizontalFromEvent"
      >
        <RecycleScroller
          class="scroller"
          :items="sorted"
          :item-size="38"
          key-field="slug"
        >
          <template #default="{ item }: { item: PaperIndexEntry }">
            <div
              class="paper-row"
              :class="{ selected: item.slug === selection.selectedSlug }"
              @mousedown="onRowMouseDown($event, item)"
              @click="selection.selectPaper(item.slug)"
              @dblclick="openInReader(item)"
              @contextmenu.prevent="openCtx($event, item)"
            >
              <div
                class="reading-dot"
                :style="{ background: STATUS_COLORS[item.reading_status] ?? STATUS_COLORS.unread }"
                :title="STATUS_LABELS[item.reading_status] ?? '未读'"
              />
              <template v-for="col in orderedVisibleCols" :key="col.id">
                <div v-if="col.id === 'title'" class="row-cell row-title" :title="displayTitle(item.title)">
                  {{ displayTitle(item.title) }}
                </div>
                <div v-else-if="col.id === 'notes'" class="row-cell row-notes" :title="canvasNotesFor(item).join(', ')">
                  <span
                    v-for="(title, index) in canvasNotesFor(item)"
                    :key="title"
                    class="note-badge"
                    :style="noteBadgeStyle(title, index)"
                  >
                    {{ title }}
                  </span>
                </div>
                <div v-else-if="col.id === 'authors'" class="row-cell" :title="item.authors.join(', ')">
                  {{ formatAuthors(item.authors) }}
                </div>
                <div v-else-if="col.id === 'venue'" class="row-cell" :title="item.venue || ''">{{ item.venue || '—' }}</div>
                <div v-else-if="col.id === 'year'" class="row-cell row-year">{{ item.year ?? '—' }}</div>
                <div v-else-if="col.id === 'added_at'" class="row-cell row-date">{{ formatDate(item.added_at) }}</div>
                <div v-else-if="col.id === 'status'" class="row-right"><StatusBadges :status="item.status" /></div>
                <div v-else-if="col.id === 'tags'" class="row-cell row-tags">
                  <template v-if="item.tags && item.tags.length > 0">
                    <span
                      v-for="tag in item.tags.slice(0, 3)"
                      :key="tag"
                      class="tag-chip"
                      :style="categoryStyleFor(tag)"
                    >
                      <span
                        v-if="categoryIconFor(tag)"
                        class="tag-icon"
                        v-html="categoryIconFor(tag)"
                      />
                      <span class="tag-text" @click="clickTag($event, tag)">{{ tag }}</span>
                      <button
                        class="tag-remove"
                        title="删除标签"
                        @click.stop="confirmRemoveTag(item, tag)"
                      >×</button>
                    </span>
                    <span v-if="item.tags.length > 3" class="tag-more">+{{ item.tags.length - 3 }}</span>
                  </template>
                </div>
                <div v-else-if="col.id === 'source'" class="row-cell row-source">
                  <span
                    class="source-chip"
                    :style="{ background: SOURCE_BG[paperSource(item)], color: SOURCE_TEXT[paperSource(item)] }"
                  >{{ SOURCE_LABEL[paperSource(item)] }}</span>
                </div>
                <div v-else-if="col.id === 'cite_count'" class="row-cell row-cite-count">
                  {{ item.cite_count != null ? item.cite_count.toLocaleString() : '—' }}
                </div>
              </template>
              <div class="hdr-trail" />
            </div>
          </template>
        </RecycleScroller>
      </div>
      <div
        ref="bottomScrollRef"
        class="bottom-scroll"
        @scroll.passive="syncHorizontalFromEvent"
      >
        <div class="bottom-scroll-spacer" />
      </div>
    </div>

    <div v-if="library.isLoading" class="loading-bar" />

    <!-- ── Context menu ──────────────────────────────────────────────────── -->
    <Teleport to="body">
      <div
        v-if="ctxMenu"
        ref="ctxMenuEl"
        class="ctx-menu"
        :class="{ 'ctx-menu--constrained': ctxMenu.constrained }"
        :style="{ left: ctxMenu.x + 'px', top: ctxMenu.y + 'px' }"
        @click.stop
      >
        <div class="ctx-item-group" @mouseenter="ctxMenu!.showColls = true" @mouseleave="ctxMenu!.showColls = false">
          <button class="ctx-item has-sub">
            {{ t('collections.addToColl') }}
            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="margin-left:auto">
              <polyline points="9 18 15 12 9 6"/>
            </svg>
          </button>
          <div v-if="ctxMenu.showColls && collectionsStore.file.collections.length === 0" class="ctx-submenu">
            <div v-if="collectionsStore.file.collections.length === 0" class="ctx-item disabled">
              {{ t('collections.noCollections') }}
            </div>
          </div>
          <CollectionCascadeMenu
            v-else-if="ctxMenu.showColls"
            :collections="contextMenuRootCollections"
            @select="addToCollection"
          />
        </div>
        <button v-if="selection.activeNav.startsWith('collection:')" class="ctx-item" @click="removeFromCurrentCollection">
          {{ t('collections.removeFromColl') }}
        </button>
        <div class="ctx-sep" />
        <button class="ctx-item" @click="copyPaperPdf(ctxMenu!.item)">
          {{ t('paper.copyPdf') }}
        </button>
        <button class="ctx-item" @click="openPaperInFinder(ctxMenu!.item)">
          {{ t('paper.openInFinder') }}
        </button>
        <div class="ctx-sep" />
        <div class="ctx-tags-section">
          <div class="ctx-tags-title">{{ t('meta.tags') }}</div>
          <div v-if="ctxMenu.item.tags && ctxMenu.item.tags.length" class="ctx-tag-chips">
            <button
              v-for="tag in ctxMenu.item.tags"
              :key="tag"
              class="ctx-tag-chip"
              :title="t('metaEdit.removeTag')"
              :style="categoryStyleFor(tag)"
              @click="removeTagFromContext(tag)"
            >
              <span
                v-if="categoryIconFor(tag)"
                class="ctx-tag-icon"
                v-html="categoryIconFor(tag)"
              />
              <span>{{ tag }}</span>
              <span class="ctx-tag-remove">×</span>
            </button>
          </div>
          <div class="ctx-tag-form">
            <input
              v-model="ctxMenu.tagInput"
              class="ctx-tag-input"
              type="text"
              :placeholder="t('metaEdit.tagPlaceholder')"
              @keydown.enter.prevent="addTagFromContext"
              @keydown.comma.prevent="addTagFromContext"
            />
            <button
              class="ctx-tag-add"
              :title="t('metaEdit.addTag')"
              :disabled="!ctxMenu.tagInput.trim()"
              @click="addTagFromContext"
            >
              +
            </button>
          </div>
          <div v-if="suggestedContextTags.length" class="ctx-tag-suggestions">
            <div class="ctx-tag-suggestions-title">{{ t('metaEdit.existingTags') }}</div>
            <div class="ctx-tag-suggestion-list">
              <button
                v-for="tag in suggestedContextTags"
                :key="tag"
                class="ctx-tag-suggestion"
                @click="addSuggestedTag(tag)"
              >
                <span
                  v-if="categoryIconFor(tag)"
                  class="ctx-tag-icon"
                  v-html="categoryIconFor(tag)"
                />
                <span>{{ tag }}</span>
              </button>
            </div>
          </div>
        </div>
        <div class="ctx-sep" />
        <div class="ctx-status-row">
          <button
            v-for="s in ['unread', 'reading', 'read']"
            :key="s"
            class="ctx-status-btn"
            :class="{ 'ctx-status-btn-active': ctxMenu!.item.reading_status === s }"
            @click="setReadingStatus(ctxMenu!.item, s)"
          >
            <span class="ctx-status-dot" :style="{ background: STATUS_COLORS[s] }" />
            {{ STATUS_LABELS[s] }}
          </button>
        </div>
        <div class="ctx-sep" />
        <div class="ctx-category-row">
          <button
            v-for="cat in PAPER_CATEGORIES"
            :key="cat.tag"
            class="ctx-category-btn"
            :class="{ 'ctx-category-btn-active': ctxMenu!.item.tags?.includes(cat.tag) }"
            :style="{
              color: ctxMenu!.item.tags?.includes(cat.tag) ? cat.color : 'var(--text-secondary)',
              background: ctxMenu!.item.tags?.includes(cat.tag) ? cat.bg : 'var(--bg-secondary)',
            }"
            @click="toggleCategoryTag(ctxMenu!.item, cat.tag)"
          >
            <span class="ctx-category-icon" v-html="cat.icon" />
            {{ cat.tag }}
          </button>
        </div>
        <div class="ctx-sep" />
        <button class="ctx-item" :disabled="isAiSummaryActive(ctxMenu!.item.slug)" @click="reExtract(ctxMenu!.item)">
          {{ t('extraction.reExtract') }}
        </button>
        <div class="ctx-sep" />
        <button
          class="ctx-item"
          :disabled="isAiSummaryButtonDisabled(ctxMenu!.item)"
          :title="isAiSummaryDone(ctxMenu!.item) ? t('paper.summaryDone') : undefined"
          @click="generateAiSummary(ctxMenu!.item)"
        >
          {{ isAiSummaryActive(ctxMenu!.item.slug) ? t('paper.summarizeAiIng') : t('paper.summarizeAi') }}
        </button>
        <button class="ctx-item" :disabled="abstractSlug === ctxMenu!.item.slug" @click="extractAbstractAi(ctxMenu!.item)">
          {{ abstractSlug === ctxMenu!.item.slug ? t('paper.extractAbstractAiIng') : t('paper.extractAbstractAi') }}
        </button>
        <button class="ctx-item" :disabled="aiMetaSlug === ctxMenu!.item.slug" @click="extractMetaAi(ctxMenu!.item)">
          {{ aiMetaSlug === ctxMenu!.item.slug ? t('paper.extractMetaAiIng') : t('paper.extractMetaAi') }}
        </button>
        <template v-if="ragStore.isConfigured">
          <div class="ctx-sep" />
          <button
            class="ctx-item"
            :disabled="vectorizingSlug === ctxMenu!.item.slug"
            @click="vectorizePaper(ctxMenu!.item)"
          >
            {{ vectorizingSlug === ctxMenu!.item.slug ? '嵌入向量库中…' : (ctxMenu!.item.status.vectorized ? '重新嵌入向量库' : '加入向量库') }}
          </button>
        </template>
        <div class="ctx-sep" />
        <button class="ctx-item danger" @click="deletePaper(ctxMenu!.item)">{{ t('paper.delete') }}</button>
      </div>
    </Teleport>
  </div>

  <!-- Drag ghost -->
  <Teleport to="body">
    <div
      v-if="dragGhostItem"
      class="paper-drag-ghost"
      :style="{ left: dragGhostPos.x + 'px', top: dragGhostPos.y + 'px' }"
    >{{ displayTitle(dragGhostItem.title) }}</div>
  </Teleport>
</template>

<style scoped>
:global(.paper-drag-ghost) {
  position: fixed;
  pointer-events: none;
  z-index: 9999;
  background: color-mix(in srgb, var(--bg-primary) 94%, var(--accent));
  border: 1.5px solid var(--accent);
  border-radius: 10px;
  padding: 6px 14px;
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  max-width: 180px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  box-shadow: 0 8px 28px rgba(0,0,0,0.22), 0 2px 6px rgba(0,0,0,0.1);
  animation: ghost-pop 0.14s cubic-bezier(0.34, 1.56, 0.64, 1) both;
}

.paper-list {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-primary);
  border-right: 1px solid var(--border-subtle);
  position: relative;
}

/* Action error toast */
.action-error-toast {
  position: absolute;
  bottom: 12px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 200;
  display: flex;
  align-items: flex-start;
  gap: 7px;
  background: #3a1a1a;
  border: 1px solid #7a2020;
  border-radius: 8px;
  padding: 8px 12px;
  max-width: 90%;
  cursor: pointer;
  box-shadow: 0 4px 16px rgba(0,0,0,0.4);
}
.action-error-toast svg { color: #f87171; flex-shrink: 0; margin-top: 1px; }
.action-error-toast span { font-size: var(--font-size-sm); color: #fca5a5; line-height: 1.4; word-break: break-word; flex: 1; }
.err-toast-enter-active, .err-toast-leave-active { transition: opacity 0.2s, transform 0.2s; }
.err-toast-enter-from, .err-toast-leave-to { opacity: 0; transform: translateX(-50%) translateY(8px); }

/* ── Column header ─────────────────────────────────────────────────────────── */
.header-shell {
  position: relative;
  height: 40px;
  flex-shrink: 0;
  display: flex;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}

.header-scroll {
  flex: 1;
  min-width: 0;
  height: 100%;
  overflow-x: hidden;
  overflow-y: hidden;
  overscroll-behavior-x: contain;
}

.list-header {
  display: grid;
  grid-template-columns: v-bind(gridCols);
  align-items: stretch;
  height: 40px;
  min-width: v-bind(tableWidthPx);
  width: max(100%, v-bind(tableWidthPx));
  background: var(--bg-secondary);
  position: relative;
  /* NO overflow:hidden - resize handles must not be clipped */
}

.hdr-spc { /* empty grid cell for reading status dot */ }
.hdr-trail { /* trailing spacer so hdr-controls never overlaps the last column */ }

.hdr-col {
  position: relative;   /* contain the resize handle */
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 500;
  color: var(--text-tertiary);
  border-right: 1px solid var(--border-subtle);
  user-select: none;
  /* NO overflow:hidden here — that would clip the resize handle */
}

/* Label gets the ellipsis, not the cell */
.hdr-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sort-arrow { font-size: 10px; flex-shrink: 0; }

/* Reorderable columns */
.optional-hdr { cursor: grab; }
.optional-hdr:hover { background: var(--bg-hover); color: var(--text-secondary); }
.optional-hdr.active { color: var(--accent); }
.list-header.is-reordering .optional-hdr {
  cursor: grabbing;
}
.optional-hdr.dragging {
  opacity: 0.35;
  background: var(--bg-hover);
}

.col-insert-line {
  position: absolute;
  top: 4px;
  bottom: 4px;
  width: 2px;
  transform: translateX(-1px);
  background: var(--accent);
  border-radius: 999px;
  box-shadow: 0 0 0 2px var(--accent-light);
  pointer-events: none;
  z-index: 20;
}

/* ── Resize handle ─────────────────────────────────────────────────────────── */
/* Sits on the right border of each resizable column.
   It's 8px wide, centred on the border: 4px inside + 4px into the next col.
   z-index keeps it above adjacent column content. */
.resize-handle {
  position: absolute;
  right: -4px;      /* half outside column boundary */
  top: 0;
  bottom: 0;
  width: 8px;
  cursor: col-resize;
  z-index: 10;
  border-radius: 4px;
  transition: background 0.15s;
}
.resize-handle:hover,
.resize-handle:active {
  background: var(--accent);
  opacity: 0.5;
}

/* Controls: count + picker */
.hdr-controls {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 0 6px 0 8px;
  border-left: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  flex-shrink: 0;
}

.list-count {
  font-size: 11px;
  color: var(--text-tertiary);
  background: var(--bg-tertiary);
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
}

.col-picker-btn {
  display: flex; align-items: center; justify-content: center;
  width: 22px; height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.col-picker-btn:hover,
.col-picker-btn.active { background: var(--bg-hover); color: var(--text-secondary); }
.col-picker-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.col-picker-btn:disabled:hover { background: transparent; }
@keyframes spin { to { transform: rotate(360deg); } }
.spin { animation: spin 0.9s linear infinite; }

.col-picker-menu {
  position: absolute;
  right: 0;
  top: calc(100% + 2px);
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  padding: 4px;
  min-width: 148px;
  z-index: 100;
}
.col-picker-title {
  font-size: 11px; color: var(--text-tertiary);
  padding: 4px 8px 6px; font-weight: 500;
  border-bottom: 1px solid var(--border-subtle); margin-bottom: 3px;
}
.col-picker-item {
  display: flex; align-items: center; gap: 8px;
  width: 100%; padding: 5px 8px;
  font-size: var(--font-size-sm); color: var(--text-primary);
  border-radius: var(--radius-sm); text-align: left;
  transition: background 0.08s;
}
.col-picker-item:hover { background: var(--bg-hover); }
.col-picker-item svg { flex-shrink: 0; color: var(--accent); }

/* ── Paper rows ────────────────────────────────────────────────────────────── */
.body-shell {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow: hidden;
}

.body-scroll {
  position: absolute;
  inset: 0 0 13px 0;
  overflow-x: auto;
  overflow-y: hidden;
  overscroll-behavior-x: contain;
  scrollbar-width: none;
}
.body-scroll::-webkit-scrollbar {
  width: 0;
  height: 0;
  display: none;
}

.scroller {
  height: 100%;
  min-width: v-bind(tableWidthPx);
  width: max(100%, v-bind(tableWidthPx));
  overflow-x: hidden;
  overflow-y: auto;
}

.bottom-scroll {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 13px;
  overflow-x: auto;
  overflow-y: hidden;
  background: var(--bg-primary);
  overscroll-behavior-x: contain;
  z-index: 5;
}
.bottom-scroll-spacer {
  width: max(100%, v-bind(tableWidthPx));
  height: 1px;
}
.bottom-scroll::-webkit-scrollbar {
  height: 9px;
}

.paper-row {
  box-sizing: border-box;
  display: grid;
  grid-template-columns: v-bind(gridCols);
  align-items: center;
  width: 100%;
  min-width: 100%;
  height: 38px;
  padding: 0;
  column-gap: 0;
  cursor: pointer;
  border-bottom: 1px solid var(--border-subtle);
  transition: background 0.1s;
}
.paper-row:hover { background: var(--bg-hover); }
.paper-row.selected {
  background: var(--bg-selected);
  box-shadow: inset 2.5px 0 0 var(--accent);
}
.paper-row:active { cursor: grabbing; }

.row-cell {
  min-width: 0; font-size: 12px; color: var(--text-tertiary);
  padding: 0 8px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.row-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  display: flex;
  align-items: center;
  gap: 6px;
  overflow: hidden;
}
.row-year { text-align: left; }
.row-date { font-size: 11px; }

.reading-dot {
  width: 7px; height: 7px; border-radius: 50%;
  justify-self: center; flex-shrink: 0;
}

.row-right {
  display: flex;
  align-items: center;
  justify-content: flex-start;
  min-width: 0;
  overflow: hidden;
  padding: 0 8px;
}

.row-tags {
  display: flex;
  align-items: center;
  gap: 3px;
  flex-wrap: nowrap;
  overflow: hidden;
}

.row-notes {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: nowrap;
}

.note-badge {
  display: inline-block;
  max-width: 128px;
  padding: 2px 7px;
  border-radius: 5px;
  border: 1px solid transparent;
  font-size: 10px;
  font-weight: 600;
  line-height: 1.25;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  box-shadow: 0 1px 2px rgba(15, 23, 42, 0.04);
  flex-shrink: 0;
}

.tag-chip {
  display: inline-flex;
  align-items: center;
  gap: 1px;
  padding: 1px 4px 1px 6px;
  background: var(--accent-light);
  color: var(--accent);
  font-size: 10px;
  font-weight: 500;
  border-radius: var(--radius-pill);
  white-space: nowrap;
  flex-shrink: 0;
  transition: background 0.1s, color 0.1s;
}
.tag-chip:hover {
  background: var(--accent);
  color: #fff;
}
.tag-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  margin-right: 2px;
}
.tag-text {
  cursor: pointer;
}
.tag-remove {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: transparent;
  color: currentColor;
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.12s, background 0.12s;
}
.tag-chip:hover .tag-remove {
  opacity: 1;
}
.tag-remove:hover {
  background: rgba(255, 255, 255, 0.25);
  color: #fff;
}

.tag-more {
  font-size: 10px;
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.row-source { justify-content: flex-start; }
.row-cite-count { font-variant-numeric: tabular-nums; }
.source-chip {
  display: inline-flex;
  align-items: center;
  padding: 1px 7px;
  font-size: 10px;
  font-weight: 600;
  border-radius: var(--radius-pill);
  white-space: nowrap;
  letter-spacing: 0.01em;
}

/* ── Empty states ──────────────────────────────────────────────────────────── */
.empty-state {
  flex: 1; display: flex; flex-direction: column;
  align-items: center; justify-content: center;
  gap: 10px; color: var(--text-tertiary); padding: 32px;
}
.empty-state p { font-size: var(--font-size-md); font-weight: 500; color: var(--text-secondary); }
.empty-state span { font-size: var(--font-size-sm); text-align: center; line-height: 1.5; }

.import-btn {
  display: flex; align-items: center; gap: 6px; margin-top: 4px;
  padding: 7px 16px; background: var(--accent); color: #fff;
  font-size: var(--font-size-sm); font-weight: 500;
  border-radius: var(--radius-pill); transition: opacity 0.12s;
}
.import-btn:hover { opacity: 0.85; }
.import-btn:disabled { opacity: 0.35; cursor: not-allowed; }

/* ── Loading bar ───────────────────────────────────────────────────────────── */
.loading-bar {
  position: absolute; bottom: 0; left: 0; right: 0; height: 2px;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: loading 1.2s ease-in-out infinite;
}
@keyframes loading { 0% { transform: translateX(-100%); } 100% { transform: translateX(100%); } }

/* ── Search results ────────────────────────────────────────────────────────── */
.search-header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 0 14px; height: 28px;
  font-size: 11px; font-weight: 500; color: var(--text-tertiary);
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-secondary); flex-shrink: 0;
}
.search-count {
  font-size: 11px; color: var(--accent);
  background: var(--accent-light); padding: 1px 7px; border-radius: var(--radius-pill);
}
.search-results { flex: 1; overflow-y: auto; }
.search-hit {
  padding: 11px 14px; border-bottom: 1px solid var(--border-subtle);
  cursor: pointer; transition: background 0.1s;
}
.search-hit:hover { background: var(--bg-hover); }
.search-hit.selected { background: var(--bg-selected); }
.hit-title { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-primary); margin-bottom: 2px; }
.hit-authors { font-size: 11px; color: var(--text-tertiary); margin-bottom: 5px; }
.hit-snippet {
  font-size: 11px; color: var(--text-secondary); line-height: 1.5;
  overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical;
}

/* ── Context menu ──────────────────────────────────────────────────────────── */
:global(.ctx-menu) {
  position: fixed; background: var(--bg-primary);
  border: 1px solid var(--border-default); border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md); padding: 5px; min-width: 180px; z-index: 2000;
}
:global(.ctx-menu.ctx-menu--constrained) {
  max-height: calc(100vh - 20px);
  overflow-y: auto;
  overscroll-behavior: contain;
}
:global(.ctx-item) {
  display: flex; align-items: center; width: 100%;
  padding: 6px 10px; font-size: var(--font-size-sm); color: var(--text-primary);
  text-align: left; border-radius: var(--radius-sm);
  transition: background 0.08s, color 0.08s; gap: 6px;
}
:global(.ctx-item:hover) { background: var(--accent); color: #fff; }
:global(.ctx-item.disabled),
:global(.ctx-item:disabled) {
  color: var(--text-tertiary);
  cursor: default;
  opacity: 0.62;
}
:global(.ctx-item.disabled:hover),
:global(.ctx-item:disabled:hover) {
  background: transparent;
  color: var(--text-tertiary);
}
:global(.ctx-item.has-sub) { justify-content: space-between; }
:global(.ctx-item.danger) { color: #e53e3e; }
:global(.ctx-item.danger:hover) { background: #e53e3e; color: #fff; }
:global(.ctx-sep) { height: 1px; background: var(--border-subtle); margin: 3px 0; }
:global(.ctx-status-dot) { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
:global(.ctx-item-active) { font-weight: 600; }
:global(.ctx-status-row) {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 4px;
  padding: 2px 0;
}
:global(.ctx-status-btn) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  min-width: 0;
  padding: 5px 7px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  transition: background 0.08s, color 0.08s;
}
:global(.ctx-status-btn:hover) {
  color: var(--accent);
  background: var(--accent-light);
}
:global(.ctx-status-btn-active) {
  color: var(--accent);
  background: var(--accent-light);
  font-weight: 600;
}
:global(.ctx-category-row) {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 4px;
  padding: 2px 0;
}
:global(.ctx-category-btn) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  min-width: 0;
  padding: 5px 7px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  border: none;
  cursor: pointer;
  transition: transform 0.08s, filter 0.08s;
}
:global(.ctx-category-btn:hover) {
  filter: brightness(0.97);
}
:global(.ctx-category-btn-active) {
  font-weight: 600;
}
:global(.ctx-category-icon) {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}
:global(.ctx-tags-section) {
  padding: 5px 6px 6px;
  min-width: 190px;
}
:global(.ctx-tags-title) {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-tertiary);
  margin-bottom: 5px;
}
:global(.ctx-tag-chips) {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-bottom: 6px;
}
:global(.ctx-tag-chip) {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 150px;
  padding: 2px 6px;
  border-radius: var(--radius-pill);
  background: var(--accent-light);
  color: var(--accent);
  font-size: 11px;
}
:global(.ctx-tag-chip span:first-child) {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
:global(.ctx-tag-icon) {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 12px;
  height: 12px;
  flex-shrink: 0;
}
:global(.ctx-tag-chip:hover) {
  background: color-mix(in srgb, #e53e3e 12%, var(--accent-light));
  color: #e53e3e;
}
:global(.ctx-tag-remove) {
  flex-shrink: 0;
  font-weight: 600;
}
:global(.ctx-tag-form) {
  display: flex;
  align-items: center;
  gap: 5px;
}
:global(.ctx-tag-input) {
  min-width: 0;
  flex: 1;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 12px;
  outline: none;
}
:global(.ctx-tag-input:focus) {
  border-color: var(--accent);
  background: var(--bg-primary);
}
:global(.ctx-tag-add) {
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  background: var(--accent);
  color: #fff;
  font-size: 17px;
  line-height: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
:global(.ctx-tag-add:disabled) {
  opacity: 0.35;
  cursor: not-allowed;
}
:global(.ctx-tag-suggestions) {
  margin-top: 7px;
  padding-top: 6px;
  border-top: 1px solid var(--border-subtle);
}
:global(.ctx-tag-suggestions-title) {
  font-size: 10px;
  font-weight: 500;
  color: var(--text-tertiary);
  margin-bottom: 5px;
}
:global(.ctx-tag-suggestion-list) {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  max-width: 190px;
  max-height: 150px;
  overflow-y: auto;
  padding-right: 4px;
}
:global(.ctx-tag-suggestion) {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  max-width: 100%;
  padding: 2px 6px;
  border-radius: var(--radius-pill);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 11px;
  line-height: 1.25;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
:global(.ctx-tag-suggestion:hover) {
  background: var(--accent-light);
  color: var(--accent);
}
:global(.ctx-item-group) { position: relative; }
:global(.ctx-submenu) {
  position: absolute; left: 100%; top: 0;
  background: var(--bg-primary); border: 1px solid var(--border-default);
  border-radius: var(--radius-lg); box-shadow: var(--shadow-md);
  padding: 5px; min-width: 210px; overflow: visible; z-index: 2001;
}
:global(.ctx-submenu.is-nested) {
  display: none;
  left: calc(100% - 1px);
  top: -5px;
}
:global(.ctx-folder-row-wrap) {
  position: relative;
}
:global(.ctx-folder-row-wrap:hover > .ctx-submenu.is-nested) {
  display: block;
}
:global(.ctx-collection-item) {
  gap: 7px;
}
:global(.ctx-collection-icon) {
  width: 15px;
  height: 17px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  line-height: 1;
  flex-shrink: 0;
}
:global(.ctx-collection-name) {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
:global(.ctx-folder-chevron) {
  margin-left: auto;
  flex-shrink: 0;
  opacity: 0.65;
}
:global(.hit-snippet mark) {
  background: color-mix(in srgb, var(--accent) 22%, transparent);
  color: inherit; border-radius: 2px; padding: 0 1px;
}
</style>

<style>
@keyframes ghost-pop {
  from { opacity: 0; transform: scale(0.88); }
  to   { opacity: 1; transform: scale(1); }
}
</style>
