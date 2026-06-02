<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useArxivStore } from '../stores/arxiv'
import { useAiStore } from '../stores/ai'
import { useCollectionsStore } from '../stores/collections'
import type { ArxivPaper, Collection } from '../types'

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
const showSortMenu = ref(false)
const sortBtnRef = ref<HTMLElement | null>(null)
const sortPopoverStyle = ref({ top: '0px', left: '0px' })
let unlistenWindowResize: UnlistenFn | null = null
let windowResizeTimer: ReturnType<typeof setTimeout> | null = null

async function saveWindowSize() {
  try {
    const appWindow = getCurrentWindow()
    const [physSize, sf] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()])
    await invoke('save_arxiv_window_size', {
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

// Single-paper analysis
const analyzingId = ref<string | null>(null)
const analyzeError = ref('')

async function analyzeSingle(paper: ArxivPaper) {
  if (analyzingId.value) return
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
const colPickerBtnRef = ref<HTMLElement | null>(null)
const colPickerStyle = ref({ top: '0px', left: '0px' })

function openColPicker(e: MouseEvent) {
  const btn = e.currentTarget as HTMLElement
  const r = btn.getBoundingClientRect()
  colPickerStyle.value = { top: `${r.bottom + 4}px`, left: `${r.left}px` }
  showColPicker.value = true
}

// Flatten collection tree depth-first for the picker list
interface FlatCol { col: Collection; depth: number }
const flatCollections = computed<FlatCol[]>(() => {
  const result: FlatCol[] = []
  function walk(parentId: string | null, depth: number) {
    const items = collectionsStore.file.collections
      .filter(c => (c.parent_id ?? null) === parentId)
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }))
    for (const col of items) {
      result.push({ col, depth })
      walk(col.id, depth + 1)
    }
  }
  walk(null, 0)
  return result
})

onMounted(async () => {
  await watchWindowSize()
  await aiStore.load()
  await store.load()
  await store.subscribeEvents()
  await collectionsStore.load()
  window.addEventListener('keydown', onKeydown)
})

onUnmounted(() => {
  store.unsubscribeEvents()
  unlistenWindowResize?.()
  if (windowResizeTimer) clearTimeout(windowResizeTimer)
  saveWindowSize()
  window.removeEventListener('keydown', onKeydown)
})

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    showSettings.value = false
    showSortMenu.value = false
    showCalendar.value = false
    showColPicker.value = false
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
watch(() => store.sortedPapers.map(p => p.arxiv_id).join('|'), () => {
  if (store.sortedPapers.length > 0 && (!selectedId.value || !store.papers.some(p => p.arxiv_id === selectedId.value))) {
    selectedId.value = store.sortedPapers[0]?.arxiv_id ?? null
  } else if (store.sortedPapers.length === 0) {
    selectedId.value = null
  }
})

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

async function refreshPapers() {
  await store.refreshInbox()
}

async function addToLibrary(paper: ArxivPaper, collectionId?: string) {
  if (paper.in_library || addingId.value) return
  showColPicker.value = false
  addingId.value = paper.arxiv_id
  addMsg.value = ''
  try {
    await store.addToLibrary(paper.arxiv_id, collectionId)
    const colName = collectionId
      ? (collectionsStore.file.collections.find(c => c.id === collectionId)?.name ?? '')
      : ''
    addMsg.value = colName ? `已添加到「${colName}」` : '已添加到文库'
    setTimeout(() => addMsg.value = '', 3000)
  } catch (e) {
    addMsg.value = String(e)
  } finally {
    addingId.value = null
  }
}

async function doFetch() {
  store.fetchMessage = ''
  await store.fetchManual()
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

function tagStyle(topic: string): Record<string, string> {
  let h = 0
  for (let i = 0; i < topic.length; i++) h = (h * 31 + topic.charCodeAt(i)) >>> 0
  const p = TAG_PALETTES[h % TAG_PALETTES.length]
  return { background: p.bg, color: p.color, borderColor: p.bg }
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
  const d = new Date(dateStr)
  return `${d.getMonth() + 1}/${d.getDate()}`
}

function formatFullDate(dateStr: string): string {
  const d = new Date(dateStr)
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`
}

const filteredPapers = computed(() => {
  if (!categoryFilter.value) return store.sortedPapers
  const f = categoryFilter.value.toLowerCase()
  return store.sortedPapers.filter(p =>
    p.categories.some(c => c.toLowerCase().includes(f)) ||
    p.title.toLowerCase().includes(f) ||
    (p.matched_topics ?? []).some(t => t.toLowerCase().includes(f))
  )
})

const groupedPapers = computed(() => {
  const map = new Map<string, ArxivPaper[]>()
  for (const p of filteredPapers.value) {
    const date = p.published.slice(0, 10)
    if (!map.has(date)) map.set(date, [])
    map.get(date)!.push(p)
  }
  return [...map.entries()]
    .sort(([a], [b]) => b.localeCompare(a))
    .map(([date, papers]) => ({ date, papers }))
})

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
    s.add(p.published.slice(0, 10))
  }
  return s
})

// Paper counts per date
const papersPerDate = computed(() => {
  const m = new Map<string, number>()
  for (const p of store.papers) {
    const d = p.published.slice(0, 10)
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
      isSelected: selectedPaper.value?.published.slice(0, 10) === dateStr,
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
  const paper = store.papers.find(p => p.published.slice(0, 10) === dateStr)
  if (paper) { selectedId.value = paper.arxiv_id; showCalendar.value = false }
}
</script>

<template>
  <div class="arxiv-view">
    <!-- Top bar -->
    <div class="arxiv-topbar" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <div class="topbar-left">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="topbar-icon">
          <path d="M12 2L2 7l10 5 10-5-10-5z"/>
          <path d="M2 17l10 5 10-5"/>
          <path d="M2 12l10 5 10-5"/>
        </svg>
        <span class="topbar-title">arXiv 推荐</span>
        <span v-if="store.loaded" class="paper-count-pill">{{ store.papers.length }} 篇</span>
      </div>
      <div class="topbar-right">
        <div v-if="store.analyzing" class="topbar-analysis-status">
          <span class="spinner" />
          <span class="analysis-progress-text">AI 分析中 {{ store.analyzeProgress.done }}/{{ store.analyzeProgress.total }}</span>
          <div class="progress-track">
            <div class="progress-fill" :style="{ width: store.analyzeProgress.total > 0 ? (store.analyzeProgress.done / store.analyzeProgress.total * 100) + '%' : '0%' }" />
          </div>
          <button class="cancel-btn" @click="store.cancelAnalysis()">取消</button>
        </div>
        <span v-if="store.scheduleStatus?.auto_fetch_enabled" class="auto-badge">
          <span class="auto-dot" />
          自动抓取已开启
        </span>
        <span v-if="store.scheduleStatus?.next_scheduled && store.scheduleStatus?.auto_fetch_enabled" class="next-label">
          下次计划: {{ store.scheduleStatus.next_scheduled }}
        </span>
        <button
          v-if="store.config.ai_analysis_enabled && !store.analyzing"
          class="tb-btn"
          @click="store.startAnalysis()"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="3"/><path d="M12 1v4M12 19v4M4.22 4.22l2.83 2.83M16.95 16.95l2.83 2.83M1 12h4M19 12h4M4.22 19.78l2.83-2.83M16.95 7.05l2.83-2.83"/>
          </svg>
          AI 分析全部
        </button>
        <button class="tb-btn topbar-fetch-btn" :disabled="store.fetching" @click="doFetch">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" :class="{ spin: store.fetching }">
            <polyline points="23 4 23 10 17 10"/>
            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
          </svg>
          {{ store.fetching ? '触发中...' : '手动触发抓取' }}
        </button>
        <button class="tb-btn" @click="showSettings = !showSettings">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
          </svg>
          设置
        </button>
      </div>
    </div>

    <!-- Status bar (progress / errors) -->
    <Transition name="status-slide">
      <div v-if="store.fetching || store.fetchMessage || addMsg" class="status-bar">
        <div v-if="store.fetching" class="status-row">
          <span class="spinner" />
          <span>{{ store.fetchMessage || '正在抓取论文...' }}</span>
        </div>
        <span v-if="store.fetchMessage && !store.fetching" class="status-error">{{ store.fetchMessage }}</span>
        <span v-if="addMsg" class="status-ok">{{ addMsg }}</span>
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
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="search-icon">
                <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
              </svg>
              <input v-model="categoryFilter" class="search-input" placeholder="按分类/标题过滤..." />
            </div>
            <!-- Unread filter toggle -->
            <button
              class="list-tool-btn"
              :class="{ active: store.filterMode === 'unread' }"
              title="只显示未读"
              @click="store.filterMode = store.filterMode === 'unread' ? 'all' : 'unread'"
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <circle cx="12" cy="12" r="9"/>
                <circle cx="12" cy="12" r="4" fill="currentColor" stroke="none"/>
              </svg>
            </button>
            <!-- Calendar toggle button -->
            <button
              ref="calBtnRef"
              class="cal-toggle-btn"
              :class="{ active: showCalendar }"
              title="日历视图"
              @click="toggleCalendar"
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="3" y="4" width="18" height="18" rx="2" ry="2"/>
                <line x1="16" y1="2" x2="16" y2="6"/>
                <line x1="8" y1="2" x2="8" y2="6"/>
                <line x1="3" y1="10" x2="21" y2="10"/>
              </svg>
            </button>
            <button
              ref="sortBtnRef"
              class="list-tool-btn"
              :class="{ active: showSortMenu }"
              title="排序"
              @click="toggleSortMenu"
            >
              <svg v-if="store.sortOrder === 'desc'" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <path d="M8 18V6"/>
                <path d="M5 9l3-3 3 3"/>
                <path d="M16 6v12"/>
                <path d="M13 15l3 3 3-3"/>
              </svg>
              <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <path d="M8 6v12"/>
                <path d="M5 15l3 3 3-3"/>
                <path d="M16 18V6"/>
                <path d="M13 9l3-3 3 3"/>
              </svg>
            </button>
            <button
              class="list-tool-btn"
              :class="{ active: store.refreshing }"
              :disabled="store.refreshing"
              title="刷新列表并删除低于阈值的论文"
              @click="refreshPapers"
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" :class="{ spin: store.refreshing }">
                <path d="M21 12a9 9 0 0 1-15.2 6.5"/>
                <path d="M3 12A9 9 0 0 1 18.2 5.5"/>
                <path d="M18 2v4h-4"/>
                <path d="M6 22v-4h4"/>
              </svg>
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

        <!-- Calendar popover (floating, anchored to button) -->
        <Teleport to="body">
          <div v-if="showCalendar" class="cal-backdrop" @click="showCalendar = false" />
          <Transition name="cal-pop" appear>
            <div v-if="showCalendar" class="calendar-popover" :style="calPopoverStyle">
              <div class="cal-header">
                <button class="cal-nav" @click="calPrevMonth">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <polyline points="15 18 9 12 15 6"/>
                  </svg>
                </button>
                <span class="cal-month-label">{{ calMonthLabel }}</span>
                <button class="cal-nav" @click="calNextMonth">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                    <polyline points="9 18 15 12 9 6"/>
                  </svg>
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
        <div class="paper-list">
          <div v-if="!store.loaded" class="list-empty">
            <span class="spinner" />
          </div>
          <div v-else-if="filteredPapers.length === 0" class="list-empty">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" style="color: var(--text-tertiary)">
              <path d="M12 2L2 7l10 5 10-5-10-5z"/>
              <path d="M2 17l10 5 10-5"/>
              <path d="M2 12l10 5 10-5"/>
            </svg>
            <p>{{ store.papers.length === 0 ? '暂无论文，点击「抓取」获取最新论文' : '无匹配结果' }}</p>
            <button v-if="store.papers.length === 0" class="fetch-btn sm" @click="doFetch">抓取</button>
          </div>

          <template v-for="group in groupedPapers" :key="group.date">
            <div class="date-header">
              <span class="date-text">{{ formatDateLabel(group.date) }}</span>
              <span class="date-count">{{ group.papers.length }}</span>
            </div>
            <div
              v-for="paper in group.papers"
              :key="paper.arxiv_id"
              class="paper-item"
              :class="{ selected: selectedId === paper.arxiv_id, unread: !paper.read }"
              @click="selectPaper(paper.arxiv_id)"
            >
              <div
                class="item-score"
                :style="{ color: scoreColor(paper.relevance_score), background: scoreBackground(paper.relevance_score) }"
              >{{ formatScore(paper.relevance_score) }}</div>
              <div class="item-body">
                <div class="item-title">{{ paper.title }}</div>
                <div class="item-footer">
                  <div class="item-meta" v-if="paper.in_library || paper.analysis_status === 'failed'">
                    <span v-if="paper.in_library" class="item-state in-library">已入库</span>
                    <span v-else-if="paper.analysis_status === 'failed'" class="item-state failed">失败</span>
                  </div>
                  <div class="item-tags">
                    <span v-for="topic in (paper.matched_topics ?? [])" :key="topic" class="tag-topic" :style="tagStyle(topic)">{{ topic }}</span>
                  </div>
                  <div v-if="paper.rating > 0" class="item-rating-mini">
                    <span v-for="i in 5" :key="i" class="star-mini" :class="{ filled: i <= paper.rating }">★</span>
                  </div>
                </div>
              </div>
            </div>
          </template>
        </div>
      </div>

      <!-- Right: paper detail panel -->
      <div class="detail-panel">
        <div v-if="!selectedPaper" class="detail-empty">
          <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" style="color: var(--text-tertiary)">
            <path d="M12 2L2 7l10 5 10-5-10-5z"/>
            <path d="M2 17l10 5 10-5"/>
            <path d="M2 12l10 5 10-5"/>
          </svg>
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

          <!-- Title -->
          <h2 class="detail-title">{{ selectedPaper.title }}</h2>

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
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
                </svg>
                {{ addingId === selectedPaper.arxiv_id ? '添加中...' : '添加到文库' }}
              </button>

              <!-- Collection picker popover -->
              <Teleport to="body">
                <div v-if="showColPicker" class="col-picker-backdrop" @click="showColPicker = false" />
                <Transition name="col-pop" appear>
                  <div v-if="showColPicker" class="col-picker-popover" :style="colPickerStyle">
                    <div class="col-picker-header">选择分类</div>
                    <div class="col-picker-list">
                      <div
                        v-if="flatCollections.length === 0"
                        class="col-picker-empty"
                      >暂无分类，请先在文库中创建</div>
                      <button
                        v-for="{ col, depth } in flatCollections"
                        :key="col.id"
                        class="col-picker-item"
                        :style="{ paddingLeft: `${10 + depth * 14}px` }"
                        @click="addToLibrary(selectedPaper, col.id)"
                      >
                        <span class="col-emoji">{{ col.emoji ?? '📁' }}</span>
                        <span class="col-name">{{ col.name }}</span>
                      </button>
                    </div>
                  </div>
                </Transition>
              </Teleport>
            </template>

            <span v-else class="in-lib-tag">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
              已在文库
            </span>
            <div class="analysis-trigger">
              <button
                class="btn-analyze"
                :class="{ analyzing: analyzingId === selectedPaper.arxiv_id }"
                :disabled="!!analyzingId || selectedPaper.analysis_status === 'done'"
                @click="analyzeSingle(selectedPaper)"
              >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" :class="{ spin: analyzingId === selectedPaper.arxiv_id }">
                  <circle cx="12" cy="12" r="3"/>
                  <path d="M12 1v4M12 19v4M4.22 4.22l2.83 2.83M16.95 16.95l2.83 2.83M1 12h4M19 12h4M4.22 19.78l2.83-2.83M16.95 7.05l2.83-2.83"/>
                </svg>
                {{ analyzingId === selectedPaper.arxiv_id ? 'AI 分析中...' : selectedPaper.analysis_status === 'done' ? '已分析' : 'AI 分析' }}
              </button>
              <span v-if="selectedPaper.analysis_status === 'failed'" class="analysis-status-tag failed">分析失败</span>
              <span v-if="analyzeError && analyzingId === null" class="analysis-error">{{ analyzeError }}</span>
            </div>
            <button class="btn-arxiv" @click="openUrl(selectedPaper.abs_url)">
              arXiv
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
                <polyline points="15 3 21 3 21 9"/>
                <line x1="10" y1="14" x2="21" y2="3"/>
              </svg>
            </button>
            <button class="btn-pdf" @click="openUrl(selectedPaper.pdf_url)">
              PDF
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
                <polyline points="14 2 14 8 20 8"/>
                <line x1="16" y1="13" x2="8" y2="13"/>
                <line x1="16" y1="17" x2="8" y2="17"/>
                <polyline points="10 9 9 9 8 9"/>
              </svg>
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
  height: 52px;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 85%, var(--bg-secondary));
  flex-shrink: 0;
  gap: 0;
}
.tl-space { width: 76px; flex-shrink: 0; }
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
.status-error { color: #ef4444; }
.status-ok { color: var(--accent); }

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
.search-row { display: flex; align-items: center; gap: 6px; }
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
  width: 28px; height: 28px; border-radius: var(--radius-md);
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
  display: flex; align-items: center; justify-content: space-between;
  padding: 8px 12px 4px;
  position: sticky; top: 0; background: var(--bg-primary); z-index: 1;
}
.date-text { font-size: 11px; font-weight: 600; color: var(--text-tertiary); letter-spacing: 0.03em; }
.date-count {
  font-size: 10px; font-weight: 600;
  background: var(--bg-tertiary); color: var(--text-tertiary);
  padding: 1px 6px; border-radius: var(--radius-pill);
  min-width: 18px; text-align: center;
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

.detail-title {
  font-size: 20px; font-weight: 700;
  line-height: 1.35; color: var(--text-primary);
  margin-bottom: 10px;
}

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

/* Collection picker popover */
.col-picker-backdrop {
  position: fixed; inset: 0; z-index: 999;
}
.col-picker-popover {
  position: fixed; z-index: 1000;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  width: 220px;
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
  max-height: 220px; overflow-y: auto;
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
.col-picker-item:hover { background: var(--bg-hover); }
.col-emoji { font-size: 13px; flex-shrink: 0; }
.col-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

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
.abstract-text {
  font-size: 13px; color: var(--text-secondary); line-height: 1.75;
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
  height: 52px;
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
  width: 28px;
  height: 28px;
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
  background: color-mix(in srgb, var(--bg-primary) 90%, var(--bg-secondary));
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
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
  width: 28px;
  height: 28px;
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
