<script setup lang="ts">
import { ref, reactive, onMounted, onUnmounted, computed, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { emitTo, type UnlistenFn } from '@tauri-apps/api/event'
import { useArxivStore } from '../stores/arxiv'
import { useAiStore } from '../stores/ai'
import { useCollectionsStore } from '../stores/collections'
import type { ArxivPaper, Collection } from '../types'
import WindowControls from '../components/WindowControls.vue'

// On Windows the native decorations are off, so we drop the macOS traffic-light
// gutter and render our own window controls (see WindowControls).
const isWindows = navigator.userAgent.toLowerCase().includes('windows')

function openUrl(url: string) {
  invoke('open_url', { url }).catch(console.error)
}

const collectionsStore = useCollectionsStore()

const store = useArxivStore()
const aiStore = useAiStore()

const showSettings = ref(false)
const addingId = ref<string | null>(null)
const addMsg = ref('')
const selectedId = ref<string | null>(null)
const categoryFilter = ref('')
const collapsedDates = ref<Set<string>>(new Set())
const showSortMenu = ref(false)
const sortBtnRef = ref<HTMLElement | null>(null)
const sortPopoverStyle = ref({ top: '0px', left: '0px' })
// Filter the list by matched-topic tags (multi-select).
const showTopicMenu = ref(false)
const topicBtnRef = ref<HTMLElement | null>(null)
const topicPopoverStyle = ref({ top: '0px', left: '0px' })
const selectedTopics = ref<Set<string>>(new Set())
let windowResizeTimer: ReturnType<typeof setTimeout> | null = null
let addMsgTimer: ReturnType<typeof setTimeout> | null = null

const ARXIV_WIN_SIZE_KEY = 'argus:arxiv:window:size'

function saveWindowSizeToStorage() {
  if (windowResizeTimer) clearTimeout(windowResizeTimer)
  windowResizeTimer = setTimeout(() => {
    try {
      localStorage.setItem(ARXIV_WIN_SIZE_KEY, JSON.stringify({
        width: window.innerWidth,
        height: window.innerHeight,
      }))
    } catch {}
  }, 400)
}

async function restoreWindowSize() {
  try {
    const raw = localStorage.getItem(ARXIV_WIN_SIZE_KEY)
    if (!raw) return
    const { width, height } = JSON.parse(raw) as { width: number; height: number }
    if (!width || !height) return
    await getCurrentWebviewWindow().setSize(new LogicalSize(
      Math.max(800, width),
      Math.max(500, height),
    ))
  } catch {}
}

async function watchWindowSize() {
  window.addEventListener('resize', saveWindowSizeToStorage)
}

// Single-paper analysis
const analyzingId = ref<string | null>(null)
const analyzeError = ref('')

async function analyzeSingle(paper: ArxivPaper) {
  if (analyzingId.value) return
  // Optimistically flip the paper to the non-terminal 'analyzing' state before
  // wiring up the watch. Re-analyzing an already-'done'/'failed' paper would
  // otherwise let the watch read the stale terminal status and clear analyzingId
  // immediately, flashing the loading state off. Now analyzingId is only cleared
  // once the store event transitions it back to a terminal state.
  const target = store.papers.find(p => p.arxiv_id === paper.arxiv_id)
  if (target) target.analysis_status = 'analyzing'
  analyzingId.value = paper.arxiv_id
  analyzeError.value = ''
  try {
    await invoke('analyze_arxiv_paper', { arxivId: paper.arxiv_id })
    // Result comes back via the arxiv-analysis event listener in the store
  } catch (e) {
    analyzeError.value = String(e)
    analyzingId.value = null
  }
}

// Clear analyzingId when the store's event marks this paper done/failed
watch(() => {
  const p = store.papers.find(p => p.arxiv_id === analyzingId.value)
  return p?.analysis_status
}, (status) => {
  if (status === 'done' || status === 'failed') {
    analyzingId.value = null
  }
})

// Collection picker
const showColPicker = ref(false)
const colPickerPos = ref({ top: 0, left: 0 })

const PICKER_PANEL_W = 196
const PICKER_PANEL_GAP = 3

function openColPicker(e: MouseEvent) {
  const btn = e.currentTarget as HTMLElement
  const r = btn.getBoundingClientRect()
  colPickerPos.value = { top: r.bottom + 4, left: r.left }
  showColPicker.value = true
  hoveredPath.value = []
  subPanelTops.value = []
  collectionsStore.load()
}

// Cascading collection picker
const colsByParent = computed(() => {
  const map = new Map<string | null, Collection[]>()
  for (const col of collectionsStore.file.collections) {
    const pid = col.parent_id ?? null
    if (!map.has(pid)) map.set(pid, [])
    map.get(pid)!.push(col)
  }
  for (const items of map.values()) {
    items.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }))
  }
  return map
})

function colChildren(parentId: string | null): Collection[] {
  return colsByParent.value.get(parentId) ?? []
}

function colHasChildren(colId: string): boolean {
  return (colsByParent.value.get(colId)?.length ?? 0) > 0
}

const hoveredPath = ref<string[]>([])
const subPanelTops = ref<number[]>([])

function onColHover(col: Collection, level: number, e: MouseEvent) {
  hoveredPath.value = [...hoveredPath.value.slice(0, level), col.id]
  if (colHasChildren(col.id)) {
    const el = e.currentTarget as HTMLElement
    const itemTop = el.getBoundingClientRect().top - colPickerPos.value.top
    subPanelTops.value = [...subPanelTops.value.slice(0, level), itemTop]
  } else {
    subPanelTops.value = subPanelTops.value.slice(0, level)
  }
}

interface CascadePanel { items: Collection[]; left: number; top: number }

const cascadePanels = computed<CascadePanel[]>(() => {
  const { left, top } = colPickerPos.value
  const result: CascadePanel[] = [{ items: colChildren(null), left, top }]
  for (let i = 0; i < hoveredPath.value.length; i++) {
    const children = colChildren(hoveredPath.value[i])
    if (children.length === 0) break
    result.push({
      items: children,
      left: left + (i + 1) * (PICKER_PANEL_W + PICKER_PANEL_GAP),
      top: top + (subPanelTops.value[i] ?? 0),
    })
  }
  return result
})

onMounted(async () => {
  await watchWindowSize()
  const restoreSizePromise = restoreWindowSize().catch(() => {})
  const aiLoadPromise = aiStore.load().catch(console.error)
  const collectionsLoadPromise = collectionsStore.load().catch(console.error)

  await store.load()
  await store.subscribeEvents()
  await restoreSizePromise
  void aiLoadPromise
  void collectionsLoadPromise
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('mousedown', onGlobalMousedown)
  emitTo('main', 'arxiv-window-opened').catch(() => {})
})

onUnmounted(() => {
  ;(store as any).unsubscribeEvents()
  if (windowResizeTimer) clearTimeout(windowResizeTimer)
  if (addMsgTimer) clearTimeout(addMsgTimer)
  window.removeEventListener('resize', saveWindowSizeToStorage)
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('mousedown', onGlobalMousedown)
  emitTo('main', 'arxiv-window-closed').catch(() => {})
})

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    if (arxivCtxMenu.value) { closeArxivCtx(); return }
    if (deletingSelectedConfirm.value) { deletingSelectedConfirm.value = false; return }
    if (hasSelection.value || activeSelectionDates.size > 0) { clearSelection(); return }
    if (deletingDate.value) { deletingDate.value = null; return }
    showSettings.value = false
    showSortMenu.value = false
    showCalendar.value = false
    showColPicker.value = false
    showTopicMenu.value = false
  }
}

const selectedPaper = computed<ArxivPaper | null>(() =>
  selectedId.value ? (store.papers.find(p => p.arxiv_id === selectedId.value) ?? null) : null
)

function selectPaper(id: string) {
  selectedId.value = selectedId.value === id ? null : id
  if (id && selectedId.value === id) {
    store.markRead(id)
  }
}

// Keep the detail panel on a real paper after refreshes/imports remove rows.
watch(() => store.papers, () => {
  const papers = store.sortedPapers
  const selectedExists = selectedId.value
    ? store.papers.some(p => p.arxiv_id === selectedId.value)
    : false
  if (papers.length > 0 && (!selectedId.value || !selectedExists)) {
    selectedId.value = papers[0]?.arxiv_id ?? null
  } else if (papers.length === 0) {
    selectedId.value = null
  }
}, { deep: false })

function toggleSortMenu() {
  if (!showSortMenu.value && sortBtnRef.value) {
    showCalendar.value = false
    const r = sortBtnRef.value.getBoundingClientRect()
    sortPopoverStyle.value = {
      top: `${r.bottom + 6}px`,
      left: `${r.left}px`,
    }
  }
  showSortMenu.value = !showSortMenu.value
}

function setSortMode(mode: 'score' | 'date' | 'rating') {
  store.sortMode = mode
}

function setSortOrder(order: 'desc' | 'asc') {
  store.sortOrder = order
}

function toggleTopicMenu() {
  if (!showTopicMenu.value && topicBtnRef.value) {
    showSortMenu.value = false
    showCalendar.value = false
    const r = topicBtnRef.value.getBoundingClientRect()
    const left = Math.min(r.left, window.innerWidth - 252)
    topicPopoverStyle.value = { top: `${r.bottom + 6}px`, left: `${Math.max(8, left)}px` }
  }
  showTopicMenu.value = !showTopicMenu.value
}

function toggleTopic(topic: string) {
  const next = new Set(selectedTopics.value)
  if (next.has(topic)) next.delete(topic)
  else next.add(topic)
  selectedTopics.value = next
}

function clearTopics() {
  selectedTopics.value = new Set()
}

async function refreshPapers() {
  await store.refreshInbox()
}

async function addToLibrary(paper: ArxivPaper, collectionId?: string) {
  if (paper.in_library || addingId.value) return
  showColPicker.value = false
  addingId.value = paper.arxiv_id
  setAddMessage('')

  // Remember which paper to select next before the store removes the current one.
  const list = filteredPapers.value
  const idx = list.findIndex(p => p.arxiv_id === paper.arxiv_id)
  const nextId = idx !== -1
    ? (list[idx + 1]?.arxiv_id ?? list[idx - 1]?.arxiv_id ?? null)
    : null

  // Switch selection before the store removes the paper so the watcher sees a valid selectedId.
  if (nextId) selectedId.value = nextId

  try {
    const slug = await store.addToLibrary(paper.arxiv_id, collectionId)
    const colName = collectionId
      ? (collectionsStore.file.collections.find(c => c.id === collectionId)?.name ?? '')
      : ''
    setAddMessage(colName ? `已添加到「${colName}」` : '已添加到文库', 3000)
    // Pass the slug so the main window can run the full AI-metadata + enrichment
    // pipeline on the freshly-added paper (it only has arXiv source metadata so far).
    emitTo('main', 'library-paper-added', { slug, title: paper.title }).catch(() => {})
  } catch (e) {
    setAddMessage(String(e))
  } finally {
    addingId.value = null
  }
}

function addToLibraryIfAllowed(paper: ArxivPaper, col: Collection) {
  if (!collectionsStore.canReceivePapers(col.id)) return
  addToLibrary(paper, col.id)
}

async function doFetch() {
  store.fetchMessage = ''
  await store.fetchManual()
}

function setAddMessage(message: string, timeoutMs = 0) {
  if (addMsgTimer) {
    clearTimeout(addMsgTimer)
    addMsgTimer = null
  }
  addMsg.value = message
  if (timeoutMs > 0 && message) {
    addMsgTimer = setTimeout(() => {
      if (addMsg.value === message) addMsg.value = ''
      addMsgTimer = null
    }, timeoutMs)
  }
}

function dismissStatusBar() {
  setAddMessage('')
  if (!store.fetching) store.fetchMessage = ''
}

const TAG_PALETTES = [
  { bg: '#dbeafe', color: '#1d4ed8' }, // blue
  { bg: '#dcfce7', color: '#15803d' }, // green
  { bg: '#ede9fe', color: '#6d28d9' }, // purple
  { bg: '#ffedd5', color: '#c2410c' }, // orange
  { bg: '#fce7f3', color: '#be185d' }, // pink
  { bg: '#ccfbf1', color: '#0f766e' }, // teal
  { bg: '#fef9c3', color: '#92400e' }, // yellow
  { bg: '#e0e7ff', color: '#4338ca' }, // indigo
]

const tagStyleCache = new Map<string, Record<string, string>>()

function tagStyle(topic: string): Record<string, string> {
  const cached = tagStyleCache.get(topic)
  if (cached) return cached
  let h = 0
  for (let i = 0; i < topic.length; i++) h = (h * 31 + topic.charCodeAt(i)) >>> 0
  const p = TAG_PALETTES[h % TAG_PALETTES.length]
  const style = { background: p.bg, color: p.color, borderColor: p.bg }
  tagStyleCache.set(topic, style)
  return style
}

function scoreColor(score: number | null): string {
  if (score === null) return 'var(--text-tertiary)'
  if (score >= 8) return '#22c55e'
  if (score >= 6) return '#f59e0b'
  return 'var(--text-tertiary)'
}

function scoreBackground(score: number | null): string {
  if (score === null) return 'var(--bg-tertiary)'
  if (score >= 8) return 'rgba(34, 197, 94, 0.12)'
  if (score >= 6) return 'rgba(245, 158, 11, 0.12)'
  return 'var(--bg-tertiary)'
}

function formatScore(score: number | null): string {
  if (score === null) return '—'
  const v = Math.min(10, Math.max(0, score))
  return Number.isInteger(v) ? String(v) : v.toFixed(1)
}

function formatDateLabel(dateStr: string): string {
  const d = new Date(dateStr + 'T12:00:00')   // noon prevents timezone off-by-one
  return `${d.getMonth() + 1}月${d.getDate()}日`
}

function formatFullDate(dateStr: string): string {
  const d = new Date(dateStr)
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`
}

function paperDisplayDate(paper: ArxivPaper): string {
  return (paper.published || paper.fetched_at).slice(0, 10)
}

// Distinct matched-topic tags across the current recommendations, with counts,
// ordered by frequency — used to populate the tag-filter menu.
const availableTopics = computed(() => {
  const counts = new Map<string, number>()
  for (const p of store.papers) {
    for (const topic of (p.matched_topics ?? [])) {
      counts.set(topic, (counts.get(topic) ?? 0) + 1)
    }
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([topic, count]) => ({ topic, count }))
})

const filteredPapers = computed(() => {
  let list = store.sortedPapers
  if (selectedTopics.value.size > 0) {
    list = list.filter(p =>
      (p.matched_topics ?? []).some(t => selectedTopics.value.has(t)))
  }
  const f = categoryFilter.value.trim().toLowerCase()
  if (f) {
    list = list.filter(p =>
      p.categories.some(c => c.toLowerCase().includes(f)) ||
      p.title.toLowerCase().includes(f) ||
      (p.matched_topics ?? []).some(t => t.toLowerCase().includes(f))
    )
  }
  return list
})

// ── Batch delete ──────────────────────────────────────────────────────────────

const deletingDate = ref<string | null>(null)    // date currently showing confirm prompt
const deleteInProgress = ref<string | null>(null) // date currently being deleted

// ── Multi-select ──────────────────────────────────────────────────────────────
// reactive(Set) gives fine-grained reactivity on .add() / .delete() / .has() / .size
const selectedPaperIds: Set<string> = reactive(new Set<string>())
const activeSelectionDates: Set<string> = reactive(new Set<string>())
const lastCheckedId = ref<string | null>(null)
const hasSelection = computed(() => selectedPaperIds.size > 0)
const deletingSelectedConfirm = ref(false)
const deletingSelectedInProgress = ref(false)

type GroupCheckState = 'none' | 'some' | 'all'

interface ArxivPaperGroup {
  date: string
  papers: ArxivPaper[]
  selectedCount: number
  checkState: GroupCheckState
  selectionActive: boolean
}

// Group by paper date, not fetch date. Refresh/backfill can write many papers
// into the same storage bucket, but the UI should reflect when the papers are from.
// Selection state is computed once per group; the template can then render
// thousands of rows without repeatedly scanning each date bucket.
const groupedPapers = computed<ArxivPaperGroup[]>(() => {
  const map = new Map<string, ArxivPaper[]>()
  for (const p of filteredPapers.value) {
    const date = paperDisplayDate(p)
    if (!map.has(date)) map.set(date, [])
    map.get(date)!.push(p)
  }
  return [...map.entries()]
    .sort(([a], [b]) => b.localeCompare(a))
    .map(([date, papers]) => {
      let selectedCount = 0
      for (const paper of papers) {
        if (selectedPaperIds.has(paper.arxiv_id)) selectedCount++
      }
      const checkState: GroupCheckState =
        selectedCount === 0 ? 'none' : selectedCount === papers.length ? 'all' : 'some'

      return {
        date,
        papers,
        selectedCount,
        checkState,
        selectionActive: activeSelectionDates.has(date) || selectedCount > 0 || deletingDate.value === date,
      }
    })
})

async function deleteGroup(group: ArxivPaperGroup) {
  deleteInProgress.value = group.date
  deletingDate.value = null
  // Recompute the ids for this date from the latest filteredPapers rather than
  // trusting the (possibly stale) group.papers snapshot captured by the
  // grouped-papers computed, so we delete exactly what's currently in the group.
  const ids = filteredPapers.value
    .filter(p => paperDisplayDate(p) === group.date)
    .map(p => p.arxiv_id)
  try {
    await invoke('delete_arxiv_papers', { arxivIds: ids })
    await store.loadInbox()
    ids.forEach(id => selectedPaperIds.delete(id))
    activeSelectionDates.delete(group.date)
    if (selectedId.value && !store.papers.find(p => p.arxiv_id === selectedId.value)) {
      selectedId.value = null
    }
  } catch (e) {
    console.error('delete_arxiv_papers by paper date failed:', e)
  } finally {
    deleteInProgress.value = null
  }
}

function toggleCheck(arxivId: string, e?: MouseEvent) {
  const flat = filteredPapers.value.map(p => p.arxiv_id)
  const a = e?.shiftKey && lastCheckedId.value ? flat.indexOf(lastCheckedId.value) : -1
  const b = flat.indexOf(arxivId)
  if (a !== -1 && b !== -1) {
    // Range-select across the currently visible (filtered) flat list. If the
    // previous anchor was filtered out (a === -1), fall through to single toggle
    // so we don't push undefined ids from a -1 start index.
    const [lo, hi] = a <= b ? [a, b] : [b, a]
    for (let i = lo; i <= hi; i++) selectedPaperIds.add(flat[i])
  } else {
    if (selectedPaperIds.has(arxivId)) {
      selectedPaperIds.delete(arxivId)
    } else {
      selectedPaperIds.add(arxivId)
    }
  }
  lastCheckedId.value = arxivId
}

function toggleDateSelection(group: ArxivPaperGroup) {
  deletingDate.value = null
  if (activeSelectionDates.has(group.date) && group.selectedCount === 0) {
    activeSelectionDates.delete(group.date)
  } else {
    activeSelectionDates.add(group.date)
    collapsedDates.value.delete(group.date)
  }
}

function toggleGroupCheck(group: ArxivPaperGroup) {
  activeSelectionDates.add(group.date)
  const allChecked = group.checkState === 'all'
  group.papers.forEach(p => allChecked ? selectedPaperIds.delete(p.arxiv_id) : selectedPaperIds.add(p.arxiv_id))
}

function selectAllVisible() {
  filteredPapers.value.forEach(p => selectedPaperIds.add(p.arxiv_id))
}

function clearSelection() {
  selectedPaperIds.clear()
  activeSelectionDates.clear()
  lastCheckedId.value = null
  deletingSelectedConfirm.value = false
  deletingDate.value = null
}

async function deleteSelected() {
  if (!hasSelection.value) {
    deletingSelectedConfirm.value = false
    return
  }
  deletingSelectedInProgress.value = true
  const ids = [...selectedPaperIds]
  try {
    await invoke('delete_arxiv_papers', { arxivIds: ids })
    await store.loadInbox()
    ids.forEach(id => selectedPaperIds.delete(id))
    lastCheckedId.value = null
    if (selectedId.value && ids.includes(selectedId.value)) selectedId.value = null
    deletingSelectedConfirm.value = false
  } catch (e) {
    console.error('delete_arxiv_papers failed:', e)
  } finally {
    deletingSelectedInProgress.value = false
  }
}

// Remove a single "not interested" paper from the recommendation list. Reuses
// the same backend command as the batch delete.
async function dismissPaper(arxivId: string) {
  closeArxivCtx()
  try {
    await invoke('delete_arxiv_papers', { arxivIds: [arxivId] })
    await store.loadInbox()
    selectedPaperIds.delete(arxivId)
    if (selectedId.value === arxivId) selectedId.value = null
  } catch (e) {
    console.error('dismiss arxiv paper failed:', e)
  }
}

// ── List item right-click menu ──────────────────────────────────────────────
const arxivCtxMenu = ref<{ x: number; y: number; arxivId: string } | null>(null)

function openArxivCtx(e: MouseEvent, arxivId: string) {
  e.preventDefault()
  const MARGIN = 10
  const menuW = 160
  const menuH = 44
  const x = Math.min(e.clientX, window.innerWidth - menuW - MARGIN)
  const y = Math.min(e.clientY, window.innerHeight - menuH - MARGIN)
  arxivCtxMenu.value = { x: Math.max(MARGIN, x), y: Math.max(MARGIN, y), arxivId }
}

function closeArxivCtx() {
  arxivCtxMenu.value = null
}

function onGlobalMousedown(e: MouseEvent) {
  if (!arxivCtxMenu.value) return
  if ((e.target as Element | null)?.closest('.arxiv-ctx-menu')) return
  closeArxivCtx()
}

// ── Calendar ──────────────────────────────────────────────────────────────────

const showCalendar = ref(false)
const calViewDate = ref(new Date())
const calBtnRef = ref<HTMLElement | null>(null)
const calPopoverStyle = ref({ top: '0px', left: '0px' })

function toggleCalendar() {
  if (!showCalendar.value && calBtnRef.value) {
    showSortMenu.value = false
    const r = calBtnRef.value.getBoundingClientRect()
    calPopoverStyle.value = {
      top: `${r.bottom + 6}px`,
      left: `${r.left}px`,
    }
  }
  showCalendar.value = !showCalendar.value
}

// Set of dates (YYYY-MM-DD) that have fetched papers
const fetchedDateSet = computed(() => {
  const s = new Set<string>()
  for (const p of store.papers) {
    s.add(paperDisplayDate(p))
  }
  return s
})

// Paper counts per date
const papersPerDate = computed(() => {
  const m = new Map<string, number>()
  for (const p of store.papers) {
    const d = paperDisplayDate(p)
    m.set(d, (m.get(d) ?? 0) + 1)
  }
  return m
})

interface CalDay {
  day: number
  dateStr: string
  hasPapers: boolean
  count: number
  isToday: boolean
  isSelected: boolean
}

const calendarGrid = computed<(CalDay | null)[]>(() => {
  const year = calViewDate.value.getFullYear()
  const month = calViewDate.value.getMonth()
  const firstDow = (new Date(year, month, 1).getDay() + 6) % 7 // Mon=0
  const daysInMonth = new Date(year, month + 1, 0).getDate()
  const todayStr = new Date().toISOString().slice(0, 10)
  const cells: (CalDay | null)[] = Array(firstDow).fill(null)
  for (let d = 1; d <= daysInMonth; d++) {
    const dateStr = `${year}-${String(month + 1).padStart(2, '0')}-${String(d).padStart(2, '0')}`
    cells.push({
      day: d,
      dateStr,
      hasPapers: fetchedDateSet.value.has(dateStr),
      count: papersPerDate.value.get(dateStr) ?? 0,
      isToday: dateStr === todayStr,
      isSelected: selectedPaper.value ? paperDisplayDate(selectedPaper.value) === dateStr : false,
    })
  }
  return cells
})

const calMonthLabel = computed(() => {
  return calViewDate.value.toLocaleDateString('zh-CN', { year: 'numeric', month: 'long' })
})

function calPrevMonth() {
  const d = new Date(calViewDate.value)
  d.setMonth(d.getMonth() - 1)
  calViewDate.value = d
}
function calNextMonth() {
  const d = new Date(calViewDate.value)
  d.setMonth(d.getMonth() + 1)
  calViewDate.value = d
}

function jumpToDate(dateStr: string) {
  // Select first paper of that date in the main list
  const paper = store.papers.find(p => paperDisplayDate(p) === dateStr)
  if (paper) { selectedId.value = paper.arxiv_id; showCalendar.value = false }
}
</script>

<template>
  <div class="arxiv-view">
    <!-- Top bar -->
    <div class="arxiv-topbar" :class="{ 'win-titlebar': isWindows }" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <div class="topbar-left" data-tauri-drag-region>
        <Icon icon="fluent:layer-24-regular" class="topbar-icon" width="16" height="16" data-tauri-drag-region />
        <span class="topbar-title" data-tauri-drag-region>arXiv 推荐</span>
        <span v-if="store.loaded" class="paper-count-pill" data-tauri-drag-region>{{ store.papers.length }} 篇</span>
        <div v-if="store.analyzing" class="topbar-analysis-status" data-tauri-drag-region>
          <span class="spinner" data-tauri-drag-region />
          <span class="analysis-progress-text" data-tauri-drag-region>AI 分析中 {{ store.analyzeProgress.done }}/{{ store.analyzeProgress.total }}</span>
          <div class="progress-track" data-tauri-drag-region>
            <div class="progress-fill" :style="{ width: store.analyzeProgress.total > 0 ? (store.analyzeProgress.done / store.analyzeProgress.total * 100) + '%' : '0%' }" data-tauri-drag-region />
          </div>
          <button class="cancel-btn" @click="store.cancelAnalysis()">取消</button>
        </div>
      </div>
      <div class="topbar-right" data-tauri-drag-region>
        <span v-if="store.scheduleStatus?.auto_fetch_enabled" class="auto-badge" data-tauri-drag-region>
          <span class="auto-dot" />
          自动抓取已开启
        </span>
        <span v-if="store.scheduleStatus?.next_scheduled && store.scheduleStatus?.auto_fetch_enabled" class="next-label" data-tauri-drag-region>
          下次计划: {{ store.scheduleStatus.next_scheduled }}
        </span>
        <button
          v-if="store.config.ai_analysis_enabled && !store.analyzing"
          class="tb-btn"
          @click="store.startAnalysis()"
        >
          <Icon icon="fluent:weather-sunny-24-regular" width="14" height="14" />
          AI 分析全部
        </button>
        <button class="tb-btn topbar-fetch-btn" :disabled="store.fetching" @click="doFetch">
          <Icon icon="fluent:arrow-sync-24-regular" width="14" height="14" :class="{ spin: store.fetching }" />
          {{ store.fetching ? '触发中...' : '手动触发抓取' }}
        </button>
        <button class="tb-btn" @click="showSettings = !showSettings">
          <Icon icon="fluent:settings-24-regular" width="14" height="14" />
          设置
        </button>
      </div>
      <WindowControls />
    </div>

    <!-- Status bar (progress / errors) -->
    <Transition name="status-slide">
      <div v-if="store.fetching || store.fetchMessage || addMsg" class="status-bar">
        <div class="status-content">
          <div v-if="store.fetching" class="status-row">
            <span class="spinner" />
            <span>{{ store.fetchMessage || '正在抓取论文...' }}</span>
          </div>
          <span v-if="store.fetchMessage && !store.fetching" class="status-error">{{ store.fetchMessage }}</span>
          <span v-if="addMsg" class="status-ok">{{ addMsg }}</span>
        </div>
        <button
          v-if="!store.fetching"
          class="status-close-btn"
          title="关闭提示"
          aria-label="关闭提示"
          @click="dismissStatusBar"
        >
          <Icon icon="fluent:dismiss-24-regular" width="13" height="13" />
        </button>
      </div>
    </Transition>

    <!-- Main: list + detail -->
    <div class="arxiv-body">
      <!-- Left: paper list panel -->
      <div class="list-panel">
        <!-- List toolbar -->
        <div class="list-toolbar">
          <div class="search-row">
            <div class="search-wrap">
              <Icon class="search-icon" icon="fluent:search-24-regular" width="12" height="12" />
              <input v-model="categoryFilter" class="search-input" placeholder="按分类/标题过滤..." />
            </div>
            <!-- Unread filter toggle -->
            <button
              class="list-tool-btn"
              :class="{ active: store.filterMode === 'unread' }"
              title="只显示未读"
              @click="store.filterMode = store.filterMode === 'unread' ? 'all' : 'unread'"
            >
              <Icon icon="fluent:record-24-regular" width="15" height="15" />
            </button>
            <!-- Tag (topic) filter -->
            <button
              ref="topicBtnRef"
              class="list-tool-btn"
              :class="{ active: showTopicMenu || selectedTopics.size > 0 }"
              title="按标签筛选"
              @click="toggleTopicMenu"
            >
              <Icon icon="fluent:tag-24-regular" width="14" height="14" />
              <span v-if="selectedTopics.size > 0" class="tool-badge">{{ selectedTopics.size }}</span>
            </button>
            <!-- Calendar toggle button -->
            <button
              ref="calBtnRef"
              class="cal-toggle-btn"
              :class="{ active: showCalendar }"
              title="日历视图"
              @click="toggleCalendar"
            >
              <Icon icon="fluent:calendar-24-regular" width="14" height="14" />
            </button>
            <button
              ref="sortBtnRef"
              class="list-tool-btn"
              :class="{ active: showSortMenu }"
              title="排序"
              @click="toggleSortMenu"
            >
              <Icon v-if="store.sortOrder === 'desc'" icon="fluent:arrow-sort-down-24-regular" width="14" height="14" />
              <Icon v-else icon="fluent:arrow-sort-up-24-regular" width="14" height="14" />
            </button>
            <button
              class="list-tool-btn"
              :class="{ active: store.refreshing }"
              :disabled="store.refreshing"
              title="刷新列表并删除低于阈值的论文"
              @click="refreshPapers"
            >
              <Icon icon="fluent:arrow-sync-24-regular" width="14" height="14" :class="{ spin: store.refreshing }" />
            </button>
          </div>
        </div>

        <Teleport to="body">
          <div v-if="showSortMenu" class="sort-menu-backdrop" @click="showSortMenu = false" />
          <Transition name="sort-pop" appear>
            <div v-if="showSortMenu" class="sort-popover" :style="sortPopoverStyle">
              <div class="sort-section-title">指标</div>
              <button class="sort-menu-item" :class="{ selected: store.sortMode === 'score' }" @click="setSortMode('score')">
                <span class="sort-check">{{ store.sortMode === 'score' ? '✓' : '' }}</span>
                <span>相关分数</span>
              </button>
              <button class="sort-menu-item" :class="{ selected: store.sortMode === 'date' }" @click="setSortMode('date')">
                <span class="sort-check">{{ store.sortMode === 'date' ? '✓' : '' }}</span>
                <span>日期</span>
              </button>
              <button class="sort-menu-item" :class="{ selected: store.sortMode === 'rating' }" @click="setSortMode('rating')">
                <span class="sort-check">{{ store.sortMode === 'rating' ? '✓' : '' }}</span>
                <span>我的评分</span>
              </button>
              <div class="sort-menu-divider" />
              <div class="sort-section-title">排序方式</div>
              <button class="sort-menu-item" :class="{ selected: store.sortOrder === 'desc' }" @click="setSortOrder('desc')">
                <span class="sort-check">{{ store.sortOrder === 'desc' ? '✓' : '' }}</span>
                <span>降序</span>
              </button>
              <button class="sort-menu-item" :class="{ selected: store.sortOrder === 'asc' }" @click="setSortOrder('asc')">
                <span class="sort-check">{{ store.sortOrder === 'asc' ? '✓' : '' }}</span>
                <span>升序</span>
              </button>
            </div>
          </Transition>
        </Teleport>

        <!-- Tag (topic) filter popover -->
        <Teleport to="body">
          <div v-if="showTopicMenu" class="sort-menu-backdrop" @click="showTopicMenu = false" />
          <Transition name="sort-pop" appear>
            <div v-if="showTopicMenu" class="topic-popover" :style="topicPopoverStyle">
              <div class="topic-popover-head">
                <span class="sort-section-title">按标签筛选</span>
                <button v-if="selectedTopics.size > 0" class="topic-clear" @click="clearTopics">清除</button>
              </div>
              <div v-if="availableTopics.length === 0" class="topic-empty">暂无标签</div>
              <div v-else class="topic-list">
                <button
                  v-for="{ topic, count } in availableTopics"
                  :key="topic"
                  class="topic-item"
                  :class="{ selected: selectedTopics.has(topic) }"
                  @click="toggleTopic(topic)"
                >
                  <span class="topic-check">{{ selectedTopics.has(topic) ? '✓' : '' }}</span>
                  <span class="topic-chip" :style="tagStyle(topic)">{{ topic }}</span>
                  <span class="topic-count">{{ count }}</span>
                </button>
              </div>
            </div>
          </Transition>
        </Teleport>

        <!-- Calendar popover (floating, anchored to button) -->
        <Teleport to="body">
          <div v-if="showCalendar" class="cal-backdrop" @click="showCalendar = false" />
          <Transition name="cal-pop" appear>
            <div v-if="showCalendar" class="calendar-popover" :style="calPopoverStyle">
              <div class="cal-header">
                <button class="cal-nav" @click="calPrevMonth">
                  <Icon icon="fluent:chevron-left-24-regular" width="12" height="12" />
                </button>
                <span class="cal-month-label">{{ calMonthLabel }}</span>
                <button class="cal-nav" @click="calNextMonth">
                  <Icon icon="fluent:chevron-right-24-regular" width="12" height="12" />
                </button>
              </div>
              <div class="cal-weekdays">
                <span v-for="w in ['一','二','三','四','五','六','日']" :key="w">{{ w }}</span>
              </div>
              <div class="cal-grid">
                <div
                  v-for="(cell, i) in calendarGrid"
                  :key="i"
                  class="cal-cell"
                  :class="{
                    empty: !cell,
                    'has-papers': cell?.hasPapers,
                    today: cell?.isToday,
                    selected: cell?.isSelected,
                    future: cell && cell.dateStr > new Date().toISOString().slice(0,10),
                  }"
                  :title="cell?.hasPapers ? `${cell.count} 篇` : undefined"
                  @click="cell?.hasPapers && jumpToDate(cell.dateStr)"
                >
                  <template v-if="cell">
                    <span class="cal-day-num">{{ cell.day }}</span>
                    <span v-if="cell.hasPapers" class="cal-star">⭐</span>
                  </template>
                </div>
              </div>
              <div class="cal-legend">
                <span class="legend-item"><span class="legend-star">⭐</span>已爬取</span>
                <span class="legend-item"><span class="legend-empty-circle">○</span>未爬取</span>
              </div>
            </div>
          </Transition>
        </Teleport>

        <!-- Paper list -->
        <div class="paper-list" :class="{ 'has-selection': hasSelection }">
          <div v-if="!store.loaded" class="list-empty">
            <span class="spinner" />
          </div>
          <div v-else-if="filteredPapers.length === 0" class="list-empty">
            <Icon icon="fluent:layer-24-regular" width="24" height="24" style="color: var(--text-tertiary)" />
            <p>{{ store.papers.length === 0 ? '暂无论文，点击「抓取」获取最新论文' : '无匹配结果' }}</p>
            <button v-if="store.papers.length === 0" class="fetch-btn sm" @click="doFetch">抓取</button>
          </div>

          <template v-for="group in groupedPapers" :key="group.date">
            <div
              class="date-header"
              :class="{
                collapsed: collapsedDates.has(group.date),
                'selection-active': group.selectionActive,
                'delete-confirming': deletingDate === group.date
              }"
              @click="collapsedDates.has(group.date) ? collapsedDates.delete(group.date) : collapsedDates.add(group.date)"
            >
              <Icon class="date-chevron" icon="fluent:chevron-down-24-regular" width="12" height="12" />
              <span class="date-text">{{ formatDateLabel(group.date) }}</span>
              <span class="date-count">{{ group.papers.length }}</span>

              <span class="date-header-actions" @click.stop>
                <button
                  class="date-action-btn date-select-btn"
                  :class="{ active: group.selectionActive }"
                  :title="group.selectionActive ? '收起本日选择' : '选择本日论文'"
                  @click="toggleDateSelection(group)"
                >
                  <Icon icon="fluent:checkbox-checked-24-regular" width="14" height="14" />
                </button>
              </span>
            </div>
            <Transition name="date-tools">
              <div v-if="group.selectionActive" class="date-selection-toolbar" @click.stop>
                <button class="date-select-all-btn" @click="toggleGroupCheck(group)">
                  <span
                    class="group-checkbox"
                    :class="{
                      checked: group.checkState === 'all',
                      indeterminate: group.checkState === 'some'
                    }"
                  >
                    <Icon v-if="group.checkState === 'all'" icon="fluent:checkmark-24-regular" width="11" height="11" />
                    <span v-else-if="group.checkState === 'some'" class="indeterminate-bar"/>
                  </span>
                  <span>本日全选</span>
                </button>
                <span class="date-selected-count">已选 {{ group.selectedCount }}</span>
                <span class="date-toolbar-spacer" />
                <button
                  class="date-delete-selected-action"
                  :disabled="!hasSelection || deletingSelectedInProgress"
                  :title="hasSelection ? '删除已选论文' : '请先选择论文'"
                  @click="deletingSelectedConfirm = true"
                >
                  <Icon icon="fluent:delete-24-regular" width="13" height="13" />
                  删除所选
                </button>
              </div>
            </Transition>
            <div class="group-papers" :class="{ collapsed: collapsedDates.has(group.date) }">
              <div class="group-papers-inner">
                <div
                  v-for="paper in group.papers"
                  :key="paper.arxiv_id"
                  class="paper-item"
                  :class="{
                    selected: selectedId === paper.arxiv_id,
                    selectable: group.selectionActive,
                    'multi-checked': selectedPaperIds.has(paper.arxiv_id),
                    unread: !paper.read
                  }"
                  @click="selectPaper(paper.arxiv_id)"
                  @contextmenu.prevent.stop="openArxivCtx($event, paper.arxiv_id)"
                >
                  <div class="item-check-zone">
                    <div
                      class="item-score"
                      :style="{ color: scoreColor(paper.relevance_score), background: scoreBackground(paper.relevance_score) }"
                    >{{ formatScore(paper.relevance_score) }}</div>
                  </div>
                  <div class="item-body">
                    <div class="item-title">{{ paper.title }}</div>
                    <div class="item-footer">
                      <div class="item-meta" v-if="paper.in_library || paper.analysis_status === 'failed'">
                        <span v-if="paper.in_library" class="item-state in-library">已入库</span>
                        <span v-else-if="paper.analysis_status === 'failed'" class="item-state failed">失败</span>
                      </div>
                      <div class="item-tags">
                        <span v-if="paper.source === 'biorxiv'" class="tag-biorxiv">bioRxiv</span>
                        <span v-for="topic in (paper.matched_topics ?? [])" :key="topic" class="tag-topic" :style="tagStyle(topic)">{{ topic }}</span>
                      </div>
                      <div v-if="paper.rating > 0" class="item-rating-mini">
                        <span v-for="i in 5" :key="i" class="star-mini" :class="{ filled: i <= paper.rating }">★</span>
                      </div>
                    </div>
                  </div>
                  <button
                    v-if="group.selectionActive"
                    class="item-checkbox"
                    :class="{ checked: selectedPaperIds.has(paper.arxiv_id) }"
                    title="选择论文"
                    @click.stop="toggleCheck(paper.arxiv_id, $event)"
                  >
                    <Icon v-if="selectedPaperIds.has(paper.arxiv_id)" icon="fluent:checkmark-24-regular" width="11" height="11" />
                  </button>
                </div>
              </div>
            </div>
          </template>
        </div>
      </div>

      <!-- Right: paper detail panel -->
      <div class="detail-panel">
        <div v-if="!selectedPaper" class="detail-empty">
          <Icon icon="fluent:layer-24-regular" width="32" height="32" style="color: var(--text-tertiary)" />
          <p>选择一篇论文查看详情</p>
        </div>

        <div v-else class="detail-scroll">
          <!-- Badges row -->
          <div class="detail-badges">
            <span
              class="score-badge"
              :style="{ color: scoreColor(selectedPaper.relevance_score), background: scoreBackground(selectedPaper.relevance_score) }"
            >{{ formatScore(selectedPaper.relevance_score) }}</span>
            <span v-for="topic in (selectedPaper.matched_topics ?? [])" :key="topic" class="badge-topic" :style="tagStyle(topic)">{{ topic }}</span>
          </div>

          <!-- Star rating -->
          <div class="detail-rating">
            <span class="rating-label">我的评分</span>
            <div class="star-row">
              <button
                v-for="i in 5"
                :key="i"
                class="star-btn"
                :class="{ filled: i <= (selectedPaper.rating ?? 0) }"
                :title="`${i} 星`"
                @click="store.ratePaper(selectedPaper.arxiv_id, i === selectedPaper.rating ? 0 : i)"
              >★</button>
            </div>
            <button
              v-if="selectedPaper.rating > 0"
              class="rating-clear-btn"
              title="清除评分"
              @click="store.ratePaper(selectedPaper.arxiv_id, 0)"
            >×</button>
          </div>

          <!-- Title + optional bioRxiv source badge -->
          <div class="detail-title-wrap">
            <h2 class="detail-title">{{ selectedPaper.title }}</h2>
            <span v-if="selectedPaper.source === 'biorxiv'" class="biorxiv-source-badge">bioRxiv</span>
          </div>

          <!-- Meta -->
          <div class="detail-meta">
            <span class="detail-authors">
              {{ selectedPaper.authors.slice(0, 6).join(', ') }}{{ selectedPaper.authors.length > 6 ? ' et al.' : '' }}
            </span>
            <span class="detail-date">发表 {{ formatFullDate(selectedPaper.published) }}</span>
          </div>

          <!-- Action buttons -->
          <div class="detail-actions">
            <template v-if="!selectedPaper.in_library">
              <!-- Single button: always opens collection picker first -->
              <button
                class="btn-add-main"
                :disabled="!!addingId"
                @click="openColPicker"
              >
                <Icon icon="fluent:add-24-regular" width="14" height="14" />
                {{ addingId === selectedPaper.arxiv_id ? '添加中...' : '添加到文库' }}
              </button>

              <!-- Collection picker: cascading flyout -->
              <Teleport to="body">
                <div v-if="showColPicker" class="col-picker-backdrop" @click="showColPicker = false" />
                <template v-if="showColPicker">
                  <Transition name="col-pop" appear>
                    <!-- Level 0: animated -->
                    <div
                      class="col-cascade-panel"
                      :style="{ left: cascadePanels[0].left + 'px', top: cascadePanels[0].top + 'px' }"
                    >
                      <div class="col-picker-header">选择分类</div>
                      <div class="col-picker-list">
                        <div v-if="cascadePanels[0].items.length === 0" class="col-picker-empty">暂无分类，请先在文库中创建</div>
                        <button
                          v-for="col in cascadePanels[0].items"
                          :key="col.id"
                          class="col-picker-item"
                          :class="{
                            'col-item-open': hoveredPath[0] === col.id,
                            'col-picker-item-disabled': !collectionsStore.canReceivePapers(col.id),
                          }"
                          @mouseenter="onColHover(col, 0, $event)"
                          @click="addToLibraryIfAllowed(selectedPaper, col)"
                        >
                          <span class="col-emoji">{{ col.emoji ?? '📁' }}</span>
                          <span class="col-name">{{ col.name }}</span>
                          <Icon v-if="colHasChildren(col.id)" class="col-chevron-right" icon="fluent:chevron-right-24-regular" width="10" height="10" />
                        </button>
                      </div>
                    </div>
                  </Transition>
                  <!-- Level 1+: appear instantly, aligned to hovered row -->
                  <div
                    v-for="(panel, idx) in cascadePanels.slice(1)"
                    :key="idx + 1"
                    class="col-cascade-panel"
                    :style="{ left: panel.left + 'px', top: panel.top + 'px' }"
                  >
                    <div class="col-picker-list">
                        <button
                          v-for="col in panel.items"
                          :key="col.id"
                          class="col-picker-item"
                          :class="{
                            'col-item-open': hoveredPath[idx + 1] === col.id,
                            'col-picker-item-disabled': !collectionsStore.canReceivePapers(col.id),
                          }"
                          @mouseenter="onColHover(col, idx + 1, $event)"
                          @click="addToLibraryIfAllowed(selectedPaper, col)"
                        >
                        <span class="col-emoji">{{ col.emoji ?? '📁' }}</span>
                        <span class="col-name">{{ col.name }}</span>
                        <Icon v-if="colHasChildren(col.id)" class="col-chevron-right" icon="fluent:chevron-right-24-regular" width="10" height="10" />
                      </button>
                    </div>
                  </div>
                </template>
              </Teleport>
            </template>

            <span v-else class="in-lib-tag">
              <Icon icon="fluent:checkmark-24-regular" width="12" height="12" />
              已在文库
            </span>
            <div class="analysis-trigger">
              <button
                class="btn-analyze"
                :class="{ analyzing: analyzingId === selectedPaper.arxiv_id }"
                :disabled="!!analyzingId || selectedPaper.analysis_status === 'done'"
                @click="analyzeSingle(selectedPaper)"
              >
                <Icon icon="fluent:weather-sunny-24-regular" width="13" height="13" :class="{ spin: analyzingId === selectedPaper.arxiv_id }" />
                {{ analyzingId === selectedPaper.arxiv_id ? 'AI 分析中...' : selectedPaper.analysis_status === 'done' ? '已分析' : 'AI 分析' }}
              </button>
              <span v-if="selectedPaper.analysis_status === 'failed'" class="analysis-status-tag failed">分析失败</span>
              <span v-if="analyzeError && analyzingId === null" class="analysis-error">{{ analyzeError }}</span>
            </div>
            <button class="btn-arxiv" :class="{ 'btn-biorxiv-link': selectedPaper.source === 'biorxiv' }" @click="openUrl(selectedPaper.abs_url)">
              {{ selectedPaper.source === 'biorxiv' ? 'bioRxiv' : 'arXiv' }}
              <Icon icon="fluent:open-24-regular" width="11" height="11" />
            </button>
            <button class="btn-pdf" @click="openUrl(selectedPaper.pdf_url)">
              PDF
              <Icon icon="fluent:document-text-24-regular" width="11" height="11" />
            </button>
            <button class="btn-dismiss" title="从推荐中移除这篇文章" @click="dismissPaper(selectedPaper.arxiv_id)">
              <Icon icon="fluent:delete-24-regular" width="12" height="12" />
              移除
            </button>
          </div>

          <!-- AI Summary -->
          <div v-if="selectedPaper.analysis_summary" class="detail-section">
            <div class="section-label">AI 摘要</div>
            <blockquote class="ai-quote">{{ selectedPaper.analysis_summary }}</blockquote>
          </div>

          <!-- Relevance reason -->
          <div v-if="selectedPaper.relevance_reason" class="detail-section">
            <div class="section-label">相关原因</div>
            <p class="section-text">{{ selectedPaper.relevance_reason }}</p>
          </div>

          <!-- Key contributions -->
          <div v-if="selectedPaper.key_contributions?.length" class="detail-section">
            <div class="section-label">关键贡献</div>
            <ul class="contrib-list">
              <li v-for="item in selectedPaper.key_contributions" :key="item">{{ item }}</li>
            </ul>
          </div>

          <!-- Abstract -->
          <div class="detail-section abstract-section">
            <div class="section-label">摘要 (ABSTRACT)</div>
            <p class="abstract-text">{{ selectedPaper.summary }}</p>
          </div>
        </div>
      </div>
    </div>

    <!-- Settings modal -->
    <Teleport to="body">
      <div v-if="showSettings" class="modal-overlay" @click.self="showSettings = false">
        <Transition name="modal-pop" appear>
          <div v-if="showSettings" class="modal-box">
            <ArxivSettingsPanel :standalone="true" @close="showSettings = false" />
          </div>
        </Transition>
      </div>
    </Teleport>

    <!-- List item right-click menu -->
    <Teleport to="body">
      <div
        v-if="arxivCtxMenu"
        class="arxiv-ctx-menu"
        :style="{ left: arxivCtxMenu.x + 'px', top: arxivCtxMenu.y + 'px' }"
        @click.stop
      >
        <button class="arxiv-ctx-item danger" @click="dismissPaper(arxivCtxMenu.arxivId)">
          <Icon icon="fluent:delete-24-regular" width="13" height="13" />
          移除
        </button>
      </div>
    </Teleport>

    <!-- Delete selected confirm -->
    <Teleport to="body">
      <div v-if="deletingSelectedConfirm" class="modal-overlay" @click.self="!deletingSelectedInProgress && (deletingSelectedConfirm = false)">
        <Transition name="modal-pop" appear>
          <div v-if="deletingSelectedConfirm" class="confirm-modal-box">
            <div class="confirm-modal-title">删除所选论文？</div>
            <p class="confirm-modal-text">将从 arXiv 推荐列表中删除已选的 {{ selectedPaperIds.size }} 篇论文。</p>
            <div class="confirm-modal-actions">
              <button class="confirm-cancel-btn" :disabled="deletingSelectedInProgress" @click="deletingSelectedConfirm = false">取消</button>
              <button class="confirm-danger-btn" :disabled="deletingSelectedInProgress" @click="deleteSelected">
                <span v-if="deletingSelectedInProgress" class="del-spinner" />
                <template v-else>确认删除</template>
              </button>
            </div>
          </div>
        </Transition>
      </div>
    </Teleport>
  </div>
</template>

<script lang="ts">
import { defineComponent } from 'vue'
import ArxivSettingsPanel from '../components/settings/ArxivSettings.vue'
export default defineComponent({ components: { ArxivSettingsPanel } })
</script>

<style scoped>
/* ── Root ── */
.arxiv-view {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-family: var(--font-sans);
}

/* ── Top bar ── */
.arxiv-topbar {
  display: flex;
  align-items: center;
  padding: 0 14px 0 0;
  height: 44px;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 85%, var(--bg-secondary));
  flex-shrink: 0;
  gap: 0;
}
.tl-space { width: 96px; flex-shrink: 0; }
/* Windows: no traffic lights, custom controls sit flush to the right edge. */
.arxiv-topbar.win-titlebar { padding-right: 0; padding-left: 12px; }
.arxiv-topbar.win-titlebar .tl-space { width: 0; }
.topbar-left { display: flex; align-items: center; gap: 7px; min-width: 0; flex: 1; }
.topbar-icon { color: var(--accent); flex-shrink: 0; }
.topbar-title { font-size: 14px; font-weight: 650; white-space: nowrap; letter-spacing: -0.01em; }
.topbar-right {
  display: flex; align-items: center; gap: 2px; flex-shrink: 0;
  border-left: 1px solid var(--border-subtle);
  padding-left: 10px;
  margin-left: 8px;
}

.auto-badge {
  display: flex; align-items: center; gap: 5px;
  font-size: 11px; color: var(--accent);
  background: var(--accent-light);
  padding: 3px 10px; border-radius: var(--radius-pill);
  white-space: nowrap;
}
.auto-dot {
  width: 6px; height: 6px; border-radius: 50%;
  background: var(--accent); animation: pulse 2s infinite;
}
@keyframes pulse { 0%,100%{opacity:1} 50%{opacity:0.4} }
.next-label { font-size: 11px; color: var(--text-tertiary); white-space: nowrap; }
.paper-count-pill {
  font-size: 11px; color: var(--text-tertiary);
  background: var(--bg-tertiary);
  padding: 2px 8px; border-radius: var(--radius-pill);
  white-space: nowrap;
}

.topbar-analysis-status {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 28px;
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  white-space: nowrap;
  margin-left: auto;
}
.topbar-analysis-status .spinner {
  width: 11px;
  height: 11px;
  border-width: 1.5px;
}
.analysis-progress-text {
  font-weight: 500;
}
.topbar-analysis-status .progress-track {
  width: 88px;
  background: color-mix(in srgb, var(--text-tertiary) 18%, transparent);
}
.topbar-analysis-status .cancel-btn {
  height: 22px;
  padding: 0 7px;
  border: none;
  background: transparent;
  font-size: 11px;
}

.tb-btn {
  display: flex; align-items: center; gap: 5px;
  font-size: 12px; color: var(--text-secondary);
  padding: 5px 10px; border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
  white-space: nowrap;
}
.tb-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.tb-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.tb-btn:disabled:hover { background: transparent; color: var(--text-secondary); }

.fetch-btn {
  display: flex; align-items: center; gap: 5px;
  font-size: 12px; font-weight: 500;
  background: var(--accent); color: #fff;
  padding: 5px 12px; border-radius: var(--radius-pill);
  transition: background 0.12s;
}
.fetch-btn:hover:not(:disabled) { background: var(--accent-hover); }
.fetch-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.fetch-btn.sm { padding: 4px 12px; }

/* ── Status bar ── */
.status-bar {
  display: flex; align-items: center; gap: 12px;
  padding: 6px 14px; font-size: 11px;
  background: var(--bg-primary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0; overflow: hidden;
}
.status-content {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
}
.status-row { display: flex; align-items: center; gap: 6px; color: var(--text-secondary); }
.progress-track {
  width: 80px; height: 3px;
  background: var(--bg-tertiary); border-radius: 2px; overflow: hidden;
}
.progress-fill { height: 100%; background: var(--accent); transition: width 0.3s; }
.cancel-btn {
  font-size: 10px; color: var(--text-tertiary);
  border: 1px solid var(--border-default);
  padding: 1px 6px; border-radius: var(--radius-pill);
  transition: background 0.1s;
}
.cancel-btn:hover { background: var(--bg-hover); }
.status-error,
.status-ok {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.status-error { color: #ef4444; }
.status-ok { color: var(--accent); }
.status-close-btn {
  width: 24px;
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--text-tertiary);
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
}
.status-close-btn:hover {
  color: var(--text-primary);
  background: var(--bg-hover);
}

.status-slide-enter-active { transition: all 0.2s ease; }
.status-slide-leave-active { transition: all 0.15s ease; }
.status-slide-enter-from { opacity: 0; transform: translateY(-8px); }
.status-slide-leave-to { opacity: 0; transform: translateY(-8px); }

/* ── Body: list + detail ── */
.arxiv-body { flex: 1; display: flex; overflow: hidden; }

/* ── List panel ── */
.list-panel {
  width: 300px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  border-right: 1px solid var(--border-subtle);
  background: var(--bg-primary);
  overflow: hidden;
}

.list-toolbar {
  padding: 8px 10px;
  border-bottom: 1px solid var(--border-subtle);
  display: flex; flex-direction: column; gap: 6px;
  flex-shrink: 0;
}
.search-row { display: flex; align-items: center; gap: 0; }
.search-wrap + .list-tool-btn { margin-left: 5px; }
.search-wrap {
  position: relative; display: flex; align-items: center; flex: 1;
  min-width: 0;
}
.search-icon { position: absolute; left: 8px; color: var(--text-tertiary); pointer-events: none; }
.search-input {
  width: 100%; padding: 5px 8px 5px 26px;
  font-size: 12px; border-radius: var(--radius-md);
  background: var(--bg-secondary); color: var(--text-primary);
  border: 1px solid var(--border-subtle);
  transition: border-color 0.15s, background 0.15s;
}
.search-input:focus { border-color: var(--accent); background: var(--bg-primary); outline: none; }
.search-input::placeholder { color: var(--text-tertiary); }

/* Calendar toggle button */
.cal-toggle-btn {
  display: flex; align-items: center; justify-content: center;
  width: 22px; height: 26px; border-radius: var(--radius-md);
  color: var(--text-tertiary); flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.cal-toggle-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.cal-toggle-btn.active { background: var(--accent-light); color: var(--accent); }

/* Calendar popover (floating) */
.cal-backdrop {
  position: fixed; inset: 0; z-index: 999;
}
.calendar-popover {
  position: fixed; z-index: 1000;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 12px 12px 10px;
  width: 240px;
}
.cal-pop-enter-active { transition: opacity 0.15s ease, transform 0.15s ease; }
.cal-pop-leave-active { transition: opacity 0.12s ease, transform 0.12s ease; }
.cal-pop-enter-from { opacity: 0; transform: translateY(-6px) scale(0.97); }
.cal-pop-leave-to   { opacity: 0; transform: translateY(-6px) scale(0.97); }

.cal-header {
  display: flex; align-items: center; justify-content: space-between;
  margin-bottom: 8px;
}
.cal-nav {
  display: flex; align-items: center; justify-content: center;
  width: 22px; height: 22px; border-radius: var(--radius-md);
  color: var(--text-secondary); transition: background 0.1s;
}
.cal-nav:hover { background: var(--bg-hover); }
.cal-month-label { font-size: 12px; font-weight: 600; color: var(--text-primary); }

.cal-weekdays {
  display: grid; grid-template-columns: repeat(7, 1fr);
  margin-bottom: 4px;
}
.cal-weekdays span {
  text-align: center; font-size: 10px;
  color: var(--text-tertiary); font-weight: 500;
  padding: 2px 0;
}

.cal-grid {
  display: grid; grid-template-columns: repeat(7, 1fr); gap: 2px;
}
.cal-cell {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  aspect-ratio: 1; border-radius: var(--radius-md);
  position: relative; cursor: default;
  transition: background 0.1s;
}
.cal-cell.has-papers { cursor: pointer; }
.cal-cell.has-papers:hover { background: var(--bg-hover); }
.cal-cell.empty { pointer-events: none; }

.cal-day-num {
  font-size: 11px; line-height: 1;
  color: var(--text-tertiary);
}
.cal-cell.has-papers .cal-day-num { color: var(--text-primary); font-weight: 600; }
.cal-cell.today .cal-day-num {
  color: var(--accent); font-weight: 700;
}
.cal-cell.today {
  background: var(--accent-light);
}
.cal-cell.future .cal-day-num { color: var(--text-tertiary); opacity: 0.4; }
.cal-cell.selected { background: var(--accent-light); }
.cal-cell.selected .cal-day-num { color: var(--accent); }

.cal-star {
  font-size: 9px; line-height: 1; margin-top: 1px;
}

.cal-legend {
  display: flex; gap: 14px; justify-content: center;
  margin-top: 8px; padding-top: 7px;
  border-top: 1px solid var(--border-subtle);
}
.legend-item { display: flex; align-items: center; gap: 4px; font-size: 10px; color: var(--text-tertiary); }
.legend-star { font-size: 11px; }
.legend-empty-circle {
  font-size: 13px; color: var(--text-tertiary); opacity: 0.5; line-height: 1;
}

.list-controls {
  display: flex; align-items: center; gap: 8px;
}
.sort-select {
  font-size: 11px; border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-secondary); color: var(--text-primary);
  padding: 3px 6px; cursor: pointer;
}
.hide-label {
  display: flex; align-items: center; gap: 4px;
  font-size: 11px; color: var(--text-secondary); cursor: pointer;
  white-space: nowrap;
}

/* Paper list (scrollable) */
.paper-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0 12px;
}
.paper-list::-webkit-scrollbar { width: 4px; }
.paper-list::-webkit-scrollbar-track { background: transparent; }
.paper-list::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 2px; }
.paper-list::-webkit-scrollbar-thumb:hover { background: var(--text-tertiary); }

.list-empty {
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  padding: 48px 16px; gap: 10px;
  color: var(--text-tertiary); text-align: center;
}
.list-empty p { font-size: 12px; color: var(--text-secondary); line-height: 1.5; }

/* Date group header */
.date-header {
  display: flex; align-items: center; gap: 5px;
  padding: 8px 12px 4px;
  position: sticky; top: 0; background: var(--bg-primary); z-index: 1;
  cursor: pointer; user-select: none;
}
.date-header:hover .date-text { color: var(--text-secondary); }
.date-header.selection-active {
  background: color-mix(in srgb, var(--accent) 5%, var(--bg-primary));
}
.date-header.delete-confirming { background: #fff5f5; }
.date-chevron {
  color: var(--text-tertiary);
  flex-shrink: 0;
  transition: transform 0.18s ease;
}
.date-header.collapsed .date-chevron { transform: rotate(-90deg); }
.date-text {
  font-size: 11px; font-weight: 600; color: var(--text-tertiary);
  letter-spacing: 0.03em; flex: 1;
  transition: color 0.12s;
}
.date-count {
  font-size: 10px; font-weight: 600;
  background: var(--bg-tertiary); color: var(--text-tertiary);
  padding: 1px 6px; border-radius: var(--radius-pill);
  min-width: 18px; text-align: center;
}

/* Delete controls in date header */
.date-header-actions {
  display: flex; align-items: center; gap: 4px; flex-shrink: 0; margin-left: 2px;
}
.date-action-btn {
  display: flex; align-items: center; justify-content: center;
  width: 24px; height: 24px; border-radius: 6px;
  background: none; border: none; cursor: pointer;
  color: var(--text-tertiary);
  transition: background 0.12s, color 0.12s;
}
.date-action-btn:hover,
.date-action-btn.active {
  background: var(--accent-light);
  color: var(--accent);
}
.date-selection-toolbar {
  display: flex; align-items: center; gap: 8px;
  min-height: 38px;
  margin: 0 0 6px;
  padding: 6px 12px;
  border-top: 1px solid var(--border-subtle);
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  overflow: hidden;
}
.date-select-all-btn {
  display: inline-flex; align-items: center; gap: 7px;
  padding: 3px 7px 3px 4px;
  border: 0; background: transparent; color: var(--text-secondary);
  font-size: 11.5px; font-weight: 600; cursor: pointer;
  border-radius: 5px;
  white-space: nowrap;
}
.date-select-all-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.date-selected-count {
  flex-shrink: 0;
  font-size: 11px; font-weight: 600; color: var(--text-tertiary);
  white-space: nowrap;
}
.date-toolbar-spacer { flex: 1; min-width: 6px; }
.delete-confirm-text {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px; font-weight: 500; color: #dc2626;
}
.date-del-confirm {
  font-size: 11px; font-weight: 600; padding: 4px 8px; border-radius: 5px;
  background: #dc2626; color: #fff; border: none; cursor: pointer;
  white-space: nowrap;
}
.date-del-confirm:hover { background: #b91c1c; }
.date-del-cancel {
  font-size: 11px; padding: 4px 8px; border-radius: 5px;
  background: var(--bg-tertiary); color: var(--text-secondary);
  border: 1px solid var(--border-subtle); cursor: pointer;
  white-space: nowrap;
}
.date-del-cancel:hover { background: var(--bg-hover); }
.date-delete-selected-action {
  display: inline-flex; align-items: center; gap: 4px;
  padding: 5px 10px; border-radius: 6px;
  background: #fff1f2; color: #dc2626;
  border: 1px solid #fecdd3; cursor: pointer;
  font-size: 11px; font-weight: 600;
  white-space: nowrap;
}
.date-delete-selected-action:hover:not(:disabled) { background: #ffe4e6; }
.date-delete-selected-action {
  border-color: color-mix(in srgb, #dc2626 42%, #fecdd3);
  font-weight: 700;
}
.date-delete-selected-action:disabled {
  cursor: not-allowed;
  color: var(--text-tertiary);
  background: var(--bg-tertiary);
  border-color: var(--border-subtle);
  opacity: 0.68;
}
.del-spinner {
  display: inline-block; width: 12px; height: 12px;
  border: 2px solid var(--border-subtle); border-top-color: #dc2626;
  border-radius: 50%; animation: spin 0.7s linear infinite;
}
.date-tools-enter-active,
.date-tools-leave-active {
  transition: opacity 0.16s ease, transform 0.16s ease, max-height 0.18s ease;
  max-height: 44px;
  overflow: hidden;
}
.date-tools-enter-from,
.date-tools-leave-to {
  opacity: 0;
  transform: translateY(-4px);
  max-height: 0;
}

/* Collapsible group */
.group-papers {
  display: grid;
  grid-template-rows: 1fr;
  transition: grid-template-rows 0.45s ease;
}
.group-papers.collapsed {
  grid-template-rows: 0fr;
}
.group-papers-inner {
  overflow: hidden;
}

/* Paper item */
.paper-item {
  position: relative;
  display: flex; align-items: flex-start; gap: 8px;
  padding: 7px 10px;
  cursor: pointer;
  border-radius: var(--radius-md);
  margin: 0 4px;
  transition: background 0.1s;
}
.paper-item:hover { background: var(--bg-hover); }
.paper-item.selected {
  background: var(--accent-light);
}
.paper-item.dimmed { opacity: 0.45; }
.paper-item.unread:not(.selected):not(:hover)::after {
  content: '';
  position: absolute;
  inset: 0;
  background: color-mix(in srgb, var(--bg-secondary) 55%, transparent);
  border-radius: inherit;
  pointer-events: none;
}

/* ── Multi-select ──────────────────────────────────────────────────────── */

/* Wrapper for the score-badge / checkbox area */
.item-check-zone {
  flex-shrink: 0;
  position: relative;
  width: 28px; height: 22px;
}

/* Score badge sits inside the zone */
.item-check-zone .item-score {
  position: absolute; inset: 0;
  font-size: 12px; font-weight: 700;
  min-width: unset; width: 100%; height: 100%;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-md);
  padding: 0;
}

.paper-item.selectable {
  padding-right: 36px;
}

.item-checkbox {
  position: absolute;
  right: 11px;
  top: 50%;
  transform: translateY(-50%);
  width: 16px; height: 16px;
  padding: 0;
  border-radius: 4px;
  border: 1.5px solid var(--text-tertiary);
  background: var(--bg-primary);
  color: #fff;
  cursor: pointer;
  display: flex; align-items: center; justify-content: center;
  transition: background 0.12s, border-color 0.12s, box-shadow 0.12s;
}
.item-checkbox:hover {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 13%, transparent);
}
.paper-item.multi-checked .item-checkbox { background: var(--accent); border-color: var(--accent); }

/* Selected item highlight */
.paper-item.multi-checked { background: color-mix(in srgb, var(--accent) 8%, transparent); }

.group-checkbox {
  width: 16px; height: 16px;
  border-radius: 4px;
  border: 1.5px solid var(--text-tertiary);
  background: var(--bg-primary);
  display: flex; align-items: center; justify-content: center;
  color: #fff;
  transition: background 0.1s, border-color 0.1s;
}
.group-checkbox.checked { background: var(--accent); border-color: var(--accent); color: #fff; }
.group-checkbox.indeterminate { background: var(--bg-tertiary); border-color: var(--text-tertiary); }
.indeterminate-bar {
  display: block; width: 7px; height: 1.5px;
  background: var(--text-secondary); border-radius: 1px;
}

/* ── Original item-score (outside check-zone, fallback) ─────────────── */
.item-score {
  flex-shrink: 0;
  font-size: 12px; font-weight: 700;
  min-width: 28px; height: 22px;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-md);
  padding: 0 4px;
}
.item-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
.item-title {
  font-size: 12px; font-weight: 500;
  color: var(--text-primary);
  line-height: 1.4;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.paper-item.selected .item-title { color: var(--accent); }

.item-footer { display: flex; align-items: center; gap: 6px; min-width: 0; overflow: hidden; }
.item-tags {
  display: flex;
  flex-wrap: nowrap;
  gap: 4px;
  overflow: hidden;
  min-width: 0;
  flex: 1;
}
.tag-biorxiv {
  font-size: 10px; padding: 1px 6px;
  border-radius: var(--radius-pill);
  font-weight: 700;
  white-space: nowrap;
  flex-shrink: 0;
  background: #d1fae5; color: #065f46;
  letter-spacing: 0.2px;
}
.tag-topic {
  font-size: 10px; padding: 1px 6px;
  border-radius: var(--radius-pill);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  max-width: 120px;
  flex-shrink: 1;
}
.item-rating-mini { display: flex; gap: 1px; margin-left: auto; flex-shrink: 0; }
.star-mini { font-size: 10px; color: var(--text-tertiary); line-height: 1; }
.star-mini.filled { color: #f59e0b; }

/* ── Detail panel ── */
.detail-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg-secondary);
}

.detail-empty {
  flex: 1; display: flex; flex-direction: column;
  align-items: center; justify-content: center; gap: 10px;
  color: var(--text-tertiary);
}
.detail-empty p { font-size: 13px; }

.detail-scroll {
  flex: 1;
  overflow-y: auto;
  padding: 24px 32px 40px;
}
.detail-scroll::-webkit-scrollbar { width: 4px; }
.detail-scroll::-webkit-scrollbar-track { background: transparent; }
.detail-scroll::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 2px; }
.detail-scroll::-webkit-scrollbar-thumb:hover { background: var(--text-tertiary); }

/* Badges */
.detail-badges { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; margin-bottom: 12px; }
.score-badge {
  font-size: 13px; font-weight: 700;
  padding: 2px 10px; border-radius: var(--radius-pill);
  min-width: 36px; text-align: center;
}
.badge-topic {
  font-size: 11px; padding: 2px 9px;
  background: var(--accent-light); color: var(--accent);
  border-radius: var(--radius-pill); font-weight: 500;
}
.badge-cat {
  font-size: 11px; padding: 2px 9px;
  background: var(--bg-primary); color: var(--text-secondary);
  border-radius: var(--radius-pill);
  border: 1px solid var(--border-default);
}

.detail-title-wrap {
  display: flex; align-items: flex-start; gap: 10px; margin-bottom: 10px;
}
.detail-title {
  font-size: 20px; font-weight: 700;
  line-height: 1.35; color: var(--text-primary);
  flex: 1;
  margin-bottom: 0;
}
.biorxiv-source-badge {
  flex-shrink: 0; margin-top: 5px;
  font-size: 10px; font-weight: 700;
  padding: 2px 7px; border-radius: 4px;
  background: #d1fae5; color: #065f46;
  letter-spacing: 0.4px;
}
.btn-biorxiv-link { color: #065f46 !important; }

.detail-meta {
  display: flex; flex-direction: column; gap: 3px;
  margin-bottom: 16px;
}
.detail-authors { font-size: 13px; color: var(--text-secondary); line-height: 1.5; }
.detail-date { font-size: 12px; color: var(--text-tertiary); }

/* Action row */
.detail-actions {
  display: flex; align-items: center; gap: 8px;
  margin-bottom: 22px; flex-wrap: wrap;
}
/* Add button */
.btn-add-main {
  display: inline-flex; align-items: center; gap: 5px;
  font-size: 12px; font-weight: 500;
  background: var(--accent); color: #fff;
  padding: 6px 14px;
  border-radius: var(--radius-pill);
  transition: background 0.12s;
}
.btn-add-main:hover:not(:disabled) { background: var(--accent-hover); }
.btn-add-main:disabled { opacity: 0.5; cursor: not-allowed; }

/* Collection picker: cascading flyout */
.col-picker-backdrop {
  position: fixed; inset: 0; z-index: 999;
}
.col-cascade-panel {
  position: fixed; z-index: 1000;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  width: 196px;
  overflow: hidden;
}
.col-pop-enter-active { transition: opacity 0.14s ease, transform 0.14s ease; }
.col-pop-leave-active { transition: opacity 0.1s ease, transform 0.1s ease; }
.col-pop-enter-from { opacity: 0; transform: translateY(-5px) scale(0.97); }
.col-pop-leave-to   { opacity: 0; transform: translateY(-5px) scale(0.97); }
.col-picker-header {
  padding: 9px 12px 7px;
  font-size: 11px; font-weight: 600;
  color: var(--text-tertiary); letter-spacing: 0.04em;
  border-bottom: 1px solid var(--border-subtle);
}
.col-picker-list {
  max-height: 240px; overflow-y: auto;
  padding: 4px 0;
}
.col-picker-list::-webkit-scrollbar { width: 3px; }
.col-picker-list::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 2px; }
.col-picker-empty { padding: 12px; font-size: 12px; color: var(--text-tertiary); text-align: center; }
.col-picker-item {
  display: flex; align-items: center; gap: 7px;
  width: 100%; padding: 7px 10px;
  font-size: 12px; color: var(--text-primary);
  text-align: left; transition: background 0.1s;
}
.col-picker-item:hover,
.col-picker-item.col-item-open { background: var(--bg-hover); }
.col-picker-item-disabled {
  color: var(--text-secondary);
  cursor: default;
}
.col-emoji { font-size: 13px; flex-shrink: 0; }
.col-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.col-chevron-right { flex-shrink: 0; color: var(--text-tertiary); }

.in-lib-tag {
  display: inline-flex; align-items: center; gap: 4px;
  font-size: 12px; font-weight: 500;
  background: var(--accent-light); color: var(--accent);
  padding: 5px 12px; border-radius: var(--radius-pill);
}
.btn-arxiv, .btn-pdf {
  display: inline-flex; align-items: center; gap: 4px;
  font-size: 12px; color: var(--text-secondary);
  border: 1px solid var(--border-default);
  padding: 5px 12px; border-radius: var(--radius-pill);
  text-decoration: none; transition: background 0.1s, color 0.1s;
}
.btn-arxiv:hover, .btn-pdf:hover { background: var(--bg-hover); color: var(--text-primary); }

.btn-dismiss {
  display: inline-flex; align-items: center; gap: 4px;
  font-size: 12px; color: var(--text-secondary);
  border: 1px solid var(--border-default);
  padding: 5px 12px; border-radius: var(--radius-pill);
  background: var(--bg-primary);
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.btn-dismiss:hover {
  background: color-mix(in srgb, #e5484d 12%, transparent);
  color: #e5484d;
  border-color: color-mix(in srgb, #e5484d 45%, transparent);
}

/* List item right-click menu */
.arxiv-ctx-menu {
  position: fixed;
  z-index: 1000;
  min-width: 132px;
  padding: 4px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
}
.arxiv-ctx-item {
  display: flex; align-items: center; gap: 8px;
  width: 100%;
  padding: 7px 10px;
  border: none; background: transparent;
  border-radius: var(--radius-sm);
  font-size: 13px; color: var(--text-primary);
  text-align: left; cursor: pointer;
  transition: background 0.1s, color 0.1s;
}
.arxiv-ctx-item:hover { background: var(--bg-hover); }
.arxiv-ctx-item.danger { color: #e5484d; }
.arxiv-ctx-item.danger:hover { background: color-mix(in srgb, #e5484d 12%, transparent); }

/* AI analysis trigger row */
.analysis-trigger {
  display: flex; align-items: center; gap: 8px;
  flex-wrap: wrap;
}
.btn-analyze {
  display: inline-flex; align-items: center; gap: 5px;
  font-size: 12px; font-weight: 500;
  border: 1px solid var(--border-default);
  color: var(--text-secondary);
  background: var(--bg-primary);
  padding: 5px 12px; border-radius: var(--radius-pill);
  transition: background 0.12s, color 0.12s, border-color 0.12s;
}
.btn-analyze:hover:not(:disabled) {
  background: var(--bg-hover); color: var(--text-primary);
  border-color: var(--accent);
}
.btn-analyze.analyzing { color: var(--accent); border-color: var(--accent); }
.btn-analyze:disabled { opacity: 0.55; cursor: not-allowed; }

.analysis-status-tag {
  font-size: 11px; padding: 2px 8px; border-radius: var(--radius-pill); font-weight: 500;
}
.analysis-status-tag.done { background: rgba(34,197,94,0.12); color: #22c55e; }
.analysis-status-tag.failed { background: rgba(239,68,68,0.1); color: #ef4444; }
.analysis-status-tag.pending { background: var(--bg-tertiary); color: var(--text-tertiary); }
.analysis-error { font-size: 11px; color: #ef4444; }

/* Sections */
.detail-section { margin-bottom: 20px; }
.section-label {
  font-size: 11px; font-weight: 600;
  color: var(--text-tertiary); letter-spacing: 0.06em; text-transform: uppercase;
  margin-bottom: 8px;
}
.ai-quote {
  margin: 0;
  padding: 12px 16px;
  background: var(--bg-primary);
  border-left: 3px solid var(--accent);
  border-radius: 0 var(--radius-md) var(--radius-md) 0;
  font-size: 13px; color: var(--text-primary);
  line-height: 1.65;
}
.section-text {
  font-size: 13px; color: var(--text-secondary); line-height: 1.65; margin: 0;
}
.contrib-list {
  margin: 0; padding-left: 18px;
  display: flex; flex-direction: column; gap: 5px;
}
.contrib-list li {
  font-size: 13px; color: var(--text-secondary); line-height: 1.6;
}
.abstract-section { padding-top: 18px; border-top: 1px solid var(--border-subtle); }
.abstract-section .section-label { font-size: 13px; }
.abstract-text {
  font-size: 15px; color: var(--text-secondary); line-height: 1.75;
  margin: 0; text-align: justify;
}

/* ── Modal ── */
.modal-overlay {
  position: fixed; inset: 0; z-index: 1000;
  background: rgba(0,0,0,0.38);
  backdrop-filter: blur(4px); -webkit-backdrop-filter: blur(4px);
  display: flex; align-items: center; justify-content: center;
}
.modal-box {
  background: var(--bg-primary);
  border-radius: 14px;
  box-shadow: var(--shadow-lg);
  border: 1px solid var(--border-subtle);
  width: min(560px, 92vw);
  max-height: min(680px, 88vh);
  display: flex; flex-direction: column; overflow: hidden;
}
.confirm-modal-box {
  width: min(360px, calc(100vw - 40px));
  padding: 18px;
  border-radius: 12px;
  border: 1px solid var(--border-subtle);
  background: var(--bg-primary);
  box-shadow: var(--shadow-lg);
}
.confirm-modal-title {
  font-size: 15px;
  font-weight: 700;
  color: var(--text-primary);
}
.confirm-modal-text {
  margin: 8px 0 18px;
  font-size: 13px;
  line-height: 1.5;
  color: var(--text-secondary);
}
.confirm-modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.confirm-cancel-btn,
.confirm-danger-btn {
  height: 32px;
  padding: 0 13px;
  border-radius: 7px;
  font-size: 12px;
  font-weight: 600;
}
.confirm-cancel-btn {
  border: 1px solid var(--border-subtle);
  background: var(--bg-tertiary);
  color: var(--text-secondary);
}
.confirm-cancel-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.confirm-danger-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 84px;
  border: 1px solid #dc2626;
  background: #dc2626;
  color: #fff;
}
.confirm-danger-btn:hover:not(:disabled) {
  background: #b91c1c;
  border-color: #b91c1c;
}
.confirm-cancel-btn:disabled,
.confirm-danger-btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.modal-pop-enter-active { animation: modal-in 0.18s cubic-bezier(0.34, 1.56, 0.64, 1); }
.modal-pop-leave-active { animation: modal-in 0.14s ease reverse; }
@keyframes modal-in {
  from { opacity: 0; transform: scale(0.94) translateY(10px); }
  to   { opacity: 1; transform: scale(1) translateY(0); }
}

/* ── Shared ── */
.spinner {
  display: inline-block; width: 12px; height: 12px;
  border: 2px solid var(--border-default); border-top-color: var(--accent);
  border-radius: 50%; animation: spin 0.7s linear infinite; flex-shrink: 0;
}
@keyframes spin { to { transform: rotate(360deg); } }
.spin { animation: spin 0.7s linear infinite; }

/* ── arXiv visual refresh ── */
.arxiv-view {
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
}

.arxiv-topbar {
  height: 44px;
  padding: 0 18px 0 0;
  background: color-mix(in srgb, var(--bg-primary) 88%, var(--bg-secondary));
  border-bottom-color: color-mix(in srgb, var(--border-subtle) 72%, var(--text-tertiary));
}

.topbar-left {
  gap: 10px;
}

.topbar-icon {
  width: 26px;
  height: 26px;
  padding: 5px;
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--accent) 11%, transparent);
  box-sizing: border-box;
}

.topbar-title {
  font-size: 15px;
  font-weight: 700;
}

.paper-count-pill {
  border: 1px solid var(--border-subtle);
  background: var(--bg-primary);
}

.paper-count-pill {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
}

.topbar-right {
  gap: 8px;
}

.auto-badge {
  height: 28px;
  padding: 0 10px;
  border: 1px solid color-mix(in srgb, var(--accent) 18%, transparent);
  background: color-mix(in srgb, var(--accent) 9%, transparent);
}

.tb-btn,
.fetch-btn {
  height: 30px;
  padding: 0 11px;
  border-radius: var(--radius-md);
}

.tb-btn {
  border: 1px solid transparent;
}

.tb-btn:hover {
  border-color: var(--border-subtle);
}

.tb-btn:disabled:hover {
  border-color: transparent;
}

.fetch-btn {
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08);
}

.status-bar {
  padding: 8px 18px;
  background: color-mix(in srgb, var(--bg-primary) 90%, var(--bg-secondary));
}

.list-panel {
  width: 322px;
  background: color-mix(in srgb, var(--bg-primary) 92%, var(--bg-secondary));
}

.list-toolbar {
  padding: 12px;
  gap: 8px;
  background: color-mix(in srgb, var(--bg-primary) 88%, var(--bg-secondary));
}

.search-input {
  height: 32px;
  padding-top: 0;
  padding-bottom: 0;
  background: var(--bg-primary);
  border-color: var(--border-default);
}

.search-input:focus {
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 10%, transparent);
}

.cal-toggle-btn {
  width: 22px;
  height: 26px;
  border: 1px solid transparent;
  border-radius: 7px;
  background: transparent;
  color: var(--text-tertiary);
}

.cal-toggle-btn:hover,
.cal-toggle-btn.active {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 9%, transparent);
}

.list-controls {
  justify-content: space-between;
}

.sort-select {
  height: 26px;
  padding: 0 8px;
  background: var(--bg-primary);
  border-color: var(--border-default);
}

.hide-label {
  padding: 3px 7px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
}

.hide-label input {
  accent-color: var(--accent);
}

.paper-list {
  padding: 6px 8px 14px;
}

.date-header {
  padding: 9px 8px 5px;
  margin: 4px 0;
  gap: 5px;
  background: color-mix(in srgb, var(--bg-primary) 90%, var(--bg-secondary));
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
  cursor: pointer;
}

.date-text {
  letter-spacing: 0;
}

.date-count {
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
}

.paper-item {
  position: relative;
  align-items: stretch;
  gap: 10px;
  min-height: 64px;
  margin: 2px 0;
  padding: 8px 10px;
  border: 1px solid transparent;
  border-left: 0;
  border-radius: var(--radius-md);
  transition: background 0.12s ease, border-color 0.12s ease, box-shadow 0.12s ease;
}

.paper-item:hover {
  background: var(--bg-primary);
  border-color: var(--border-subtle);
}

.paper-item.selected {
  background: color-mix(in srgb, var(--accent) 11%, var(--bg-primary));
  border-color: color-mix(in srgb, var(--accent) 26%, var(--border-subtle));
}
.paper-item.unread:not(.selected):not(:hover)::after {
  content: '';
  position: absolute;
  inset: 0;
  background: color-mix(in srgb, var(--bg-secondary) 55%, transparent);
  border-radius: inherit;
  pointer-events: none;
}

.item-score {
  align-self: stretch;
  width: 48px;
  min-width: 48px;
  min-height: 48px;
  height: auto;
  border-radius: var(--radius-md);
  padding: 0;
  font-size: 13px;
}

.item-body {
  gap: 5px;
  justify-content: center;
  padding: 1px 0;
}

.item-title {
  font-size: 12.5px;
  font-weight: 600;
  line-height: 1.34;
}

.item-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 15px;
  font-size: 10.5px;
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.item-state {
  padding: 1px 5px;
  border-radius: var(--radius-pill);
  font-weight: 600;
  line-height: 1.2;
}

.item-state.in-library {
  color: #16803a;
  background: rgba(34, 197, 94, 0.12);
}

.item-state.failed {
  color: #dc2626;
  background: rgba(239, 68, 68, 0.10);
}

.item-state.biorxiv {
  color: #065f46;
  background: #d1fae5;
  font-weight: 600;
}

.tag-topic {
  padding: 1px 6px;
  border-radius: 6px;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  max-width: 120px;
  flex-shrink: 1;
}

/* ── Star rating (detail panel) ── */
.detail-rating {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
.rating-label {
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
  font-weight: 500;
}
.star-row {
  display: flex;
  gap: 2px;
}
.star-btn {
  font-size: 20px;
  color: var(--border-default);
  line-height: 1;
  padding: 0 1px;
  transition: color 0.1s, transform 0.1s;
  background: none;
  border: none;
  cursor: pointer;
}
.star-btn.filled { color: #f59e0b; }
.star-row:hover .star-btn { color: #f59e0b; }
.star-row:hover .star-btn:hover ~ .star-btn { color: var(--border-default); }
.star-btn:hover { transform: scale(1.15); }
.rating-clear-btn {
  font-size: 13px;
  color: var(--text-tertiary);
  padding: 1px 5px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  background: none;
  cursor: pointer;
  line-height: 1;
  transition: background 0.1s, color 0.1s;
}
.rating-clear-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.detail-panel {
  background: var(--bg-primary);
}

.detail-scroll {
  width: 100%;
  max-width: none;
  padding: 30px 40px 48px;
}

.detail-badges {
  gap: 7px;
  margin-bottom: 14px;
}

.score-badge,
.badge-topic,
.badge-cat {
  height: 26px;
  display: inline-flex;
  align-items: center;
  border-radius: var(--radius-md);
}

.score-badge {
  border: 1px solid transparent;
}

.badge-topic {
  border: 1px solid color-mix(in srgb, var(--accent) 18%, transparent);
  background: color-mix(in srgb, var(--accent) 9%, transparent);
}

.badge-cat {
  background: color-mix(in srgb, var(--bg-secondary) 62%, var(--bg-primary));
}

.detail-title {
  margin: 0 0 12px;
  font-size: 24px;
  line-height: 1.28;
  letter-spacing: 0;
}

.detail-meta {
  padding-bottom: 18px;
  margin-bottom: 18px;
  border-bottom: 1px solid var(--border-subtle);
}

.detail-authors {
  font-size: 14px;
}

.detail-actions {
  gap: 8px;
  margin-bottom: 28px;
}

.btn-add-main,
.btn-analyze,
.btn-arxiv,
.btn-pdf,
.in-lib-tag {
  height: 34px;
  padding: 0 14px;
  border-radius: var(--radius-md);
}

.btn-add-main {
  border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
  box-shadow: 0 2px 6px color-mix(in srgb, var(--accent) 18%, transparent);
}

.btn-analyze,
.btn-arxiv,
.btn-pdf {
  background: var(--bg-primary);
}

.btn-analyze:hover:not(:disabled),
.btn-arxiv:hover,
.btn-pdf:hover {
  border-color: color-mix(in srgb, var(--accent) 32%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 7%, var(--bg-primary));
}

.btn-analyze:disabled {
  opacity: 1;
  color: var(--text-tertiary);
  background: var(--bg-secondary);
  border-color: var(--border-subtle);
  box-shadow: none;
}

.in-lib-tag {
  color: #16803a;
  background: rgba(34, 197, 94, 0.12);
  border: 1px solid rgba(34, 197, 94, 0.18);
}

.analysis-status-tag {
  height: 24px;
  display: inline-flex;
  align-items: center;
}

.detail-section {
  padding-top: 20px;
  margin-bottom: 22px;
  border-top: 1px solid var(--border-subtle);
}

.section-label {
  margin-bottom: 9px;
  letter-spacing: 0.02em;
}

.ai-quote {
  padding: 14px 16px;
  border: 1px solid var(--border-subtle);
  border-left: 3px solid var(--accent);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--accent) 4%, var(--bg-secondary));
}

.section-text,
.contrib-list li,
.abstract-text {
  font-size: 13.5px;
  line-height: 1.72;
}

.contrib-list li::marker {
  color: var(--accent);
}

.abstract-section {
  padding-top: 20px;
}

.abstract-text {
  text-align: left;
}

.list-controls {
  display: contents;
  gap: 6px;
}

.list-tool-btn {
  width: 22px;
  height: 26px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid transparent;
  border-radius: 7px;
  background: transparent;
  color: var(--text-tertiary);
  transition: background 0.12s ease, color 0.12s ease, border-color 0.12s ease;
}

.list-tool-btn:hover:not(:disabled),
.list-tool-btn.active {
  color: var(--accent);
  border-color: transparent;
  background: color-mix(in srgb, var(--accent) 9%, transparent);
}

.list-tool-btn:disabled {
  cursor: not-allowed;
  opacity: 0.62;
}

.sort-menu-backdrop {
  position: fixed;
  inset: 0;
  z-index: 999;
}

.list-tool-btn { position: relative; }
.tool-badge {
  position: absolute;
  top: -3px;
  right: -3px;
  min-width: 14px;
  height: 14px;
  padding: 0 3px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  background: var(--accent);
  color: #fff;
  font-size: 9px;
  font-weight: 700;
  line-height: 1;
}
.topic-popover {
  position: fixed;
  z-index: 1000;
  width: 240px;
  max-height: 360px;
  overflow-y: auto;
  padding: 8px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  box-shadow: var(--shadow-lg);
}
.topic-popover-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px 6px 6px;
}
.topic-clear {
  border: none;
  background: transparent;
  color: var(--accent);
  font-size: 12px;
  cursor: pointer;
}
.topic-empty {
  padding: 16px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: 13px;
}
.topic-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.topic-item {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  padding: 5px 8px;
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  cursor: pointer;
  text-align: left;
}
.topic-item:hover,
.topic-item.selected { background: var(--bg-hover); }
.topic-check {
  width: 12px;
  flex-shrink: 0;
  color: var(--accent);
  font-size: 12px;
}
.topic-chip {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  font-size: 12px;
}
.topic-count {
  flex-shrink: 0;
  color: var(--text-tertiary);
  font-size: 11px;
}
.sort-popover {
  position: fixed;
  z-index: 1000;
  width: 136px;
  padding: 8px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  box-shadow: var(--shadow-lg);
}

.sort-pop-enter-active,
.sort-pop-leave-active {
  transition: opacity 0.14s ease, transform 0.14s ease;
}

.sort-pop-enter-from,
.sort-pop-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.98);
}

.sort-section-title {
  padding: 5px 10px;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
}

.sort-menu-item {
  width: 100%;
  height: 32px;
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 0 10px;
  border-radius: var(--radius-md);
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 500;
  text-align: left;
}

.sort-menu-item:hover,
.sort-menu-item.selected {
  background: color-mix(in srgb, var(--bg-secondary) 80%, var(--bg-primary));
}

.sort-menu-item.selected {
  color: color-mix(in srgb, var(--accent) 68%, var(--text-primary));
  font-weight: 700;
}

.sort-check {
  width: 14px;
  display: inline-flex;
  justify-content: center;
  color: var(--accent);
  font-weight: 700;
}

.sort-menu-divider {
  height: 1px;
  margin: 7px 4px;
  background: var(--border-subtle);
}

.detail-scroll {
  padding: 28px 36px 44px;
  user-select: text;
  -webkit-user-select: text;
  cursor: text;
}

.detail-scroll button,
.detail-scroll .btn-add-main,
.detail-scroll .btn-analyze,
.detail-scroll .btn-arxiv,
.detail-scroll .btn-pdf,
.detail-scroll .in-lib-tag,
.detail-badges {
  user-select: none;
  -webkit-user-select: none;
  cursor: default;
}

.detail-title {
  font-size: 21px;
  line-height: 1.32;
}

.detail-authors {
  font-size: 13.5px;
}

.detail-date {
  font-size: 12px;
}

.btn-add-main,
.btn-analyze,
.btn-arxiv,
.btn-pdf,
.in-lib-tag {
  font-size: 12px;
}

.section-label {
  font-size: 11px;
}

.ai-quote,
.section-text,
.contrib-list li,
.abstract-text {
  font-size: 13.5px;
  line-height: 1.7;
}
</style>
