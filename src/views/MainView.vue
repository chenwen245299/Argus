<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useImportStore } from '../stores/import'
import { useSettingsStore } from '../stores/settings'
import { useCollectionsStore } from '../stores/collections'
import { useAiStore } from '../stores/ai'
import { useCliStore } from '../stores/cli'
import { useSelectionStore } from '../stores/selection'
import Toolbar from '../components/Toolbar.vue'
import LeftSidebar from '../components/LeftSidebar.vue'
import PaperList from '../components/PaperList.vue'
import PdfViewer from '../components/PdfViewer.vue'
import CanvasPanel from '../components/CanvasPanel.vue'
import TabBar from '../components/TabBar.vue'
import RightSidebar from '../components/RightSidebar.vue'
import SettingsModal from '../components/SettingsModal.vue'

const { t } = useI18n()
const libraryStore = useLibraryStore()
const readerStore = useReaderStore()
const importStore = useImportStore()
const settingsStore = useSettingsStore()
const collectionsStore = useCollectionsStore()
const aiStore = useAiStore()
const cliStore = useCliStore()
const selectionStore = useSelectionStore()

// ── Window size persistence ────────────────────────────────────────────────────
const WIN_SIZE_KEY = 'argus:window:size'
let winResizeTimer: ReturnType<typeof setTimeout> | null = null

function onWinResize() {
  if (winResizeTimer) clearTimeout(winResizeTimer)
  winResizeTimer = setTimeout(() => {
    try {
      localStorage.setItem(WIN_SIZE_KEY, JSON.stringify({
        width: window.innerWidth,
        height: window.innerHeight,
      }))
    } catch {}
  }, 400)
}

async function restoreWindowSize() {
  try {
    const raw = localStorage.getItem(WIN_SIZE_KEY)
    if (!raw) return
    const { width, height } = JSON.parse(raw) as { width: number; height: number }
    if (!width || !height) return
    await getCurrentWebviewWindow().setSize(new LogicalSize(
      Math.max(900, width),
      Math.max(600, height),
    ))
  } catch {}
}

const showSettings = ref(false)
const settingsAiSection = ref(false)  // true = open settings on AI Services tab
const MAIN_LEFT_WIDTH_KEY = 'argus:layout:left-width'
const MAIN_RIGHT_WIDTH_KEY = 'argus:layout:right-width'
const MAIN_RIGHT_VISIBLE_KEY = 'argus:layout:right-visible'
const MAIN_RIGHT_TAB_KEY = 'argus:layout:right-tab'
const PAPER_TABS = ['notes', 'highlights', 'ai', 'metadata']
const MIN_LEFT_WIDTH = 140
const MAX_LEFT_WIDTH = 360
const DEFAULT_LEFT_WIDTH = 200
const MIN_RIGHT_WIDTH = 300
const MAX_RIGHT_WIDTH = 560
const DEFAULT_RIGHT_WIDTH = MIN_RIGHT_WIDTH

function loadLayoutNumber(key: string, fallback: number, min: number, max: number) {
  try {
    const raw = Number(localStorage.getItem(key))
    if (Number.isFinite(raw) && raw > 0) return Math.min(max, Math.max(min, raw))
  } catch {}
  return fallback
}

function loadLayoutBoolean(key: string, fallback: boolean) {
  try {
    const raw = localStorage.getItem(key)
    if (raw === 'true') return true
    if (raw === 'false') return false
  } catch {}
  return fallback
}

function loadSidebarTab() {
  try {
    const raw = localStorage.getItem(MAIN_RIGHT_TAB_KEY)
    if (raw && PAPER_TABS.includes(raw)) return raw
  } catch {}
  return 'metadata'
}

function saveLayoutNumber(key: string, value: number) {
  try {
    localStorage.setItem(key, String(Math.round(value)))
  } catch {}
}

const rightSidebarVisible = ref(loadLayoutBoolean(MAIN_RIGHT_VISIBLE_KEY, true))
const sidebarTab = ref<string>(loadSidebarTab())
const pdfViewerRef = ref<{ closeToList: () => void } | null>(null)
const showCanvas = ref(false)
const showLibraryLoading = computed(() =>
  libraryStore.isRestoringLibrary || (!libraryStore.currentPath && libraryStore.isLoading)
)

function onOpenCanvas() {
  showCanvas.value = true
  rightSidebarVisible.value = true
}

function onCanvasSelectPaper(slug: string) {
  selectionStore.selectPaper(slug)
  rightSidebarVisible.value = true
  // Ensure the sidebar is on a tab that shows paper-level content
  if (!PAPER_TABS.includes(sidebarTab.value)) {
    sidebarTab.value = 'metadata'
  }
}

function onSwitchSidebarTab(event: Event) {
  const { tab } = (event as CustomEvent<{ tab: string }>).detail ?? {}
  if (tab && PAPER_TABS.includes(tab)) {
    sidebarTab.value = tab
    rightSidebarVisible.value = true
  }
}

function openSettingsOnAi() {
  settingsAiSection.value = true
  showSettings.value = true
}

// ── Keyboard shortcuts ─────────────────────────────────────────────────────────
function onKeydown(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && e.key === ',') {
    e.preventDefault()
    showSettings.value = !showSettings.value
  }
  // Dev-only: Cmd+Option+I opens WebView inspector
  if (import.meta.env.DEV && e.metaKey && e.altKey && e.key === 'i') {
    e.preventDefault()
    const webview = getCurrentWebviewWindow() as unknown as { openDevtools?: () => void }
    webview.openDevtools?.()
  }
}
onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))

// ── Drag-drop ──────────────────────────────────────────────────────────────────
const isDragging = ref(false)
const isPaperDragging = ref(false)
let unlistenDragDrop: (() => void) | null = null
let unlistenOpenPaper: UnlistenFn | null = null

function onPaperDragStart() {
  isPaperDragging.value = true
  isDragging.value = false
}

function onPaperDragEnd() {
  isPaperDragging.value = false
  isDragging.value = false
}

onMounted(async () => {
  window.addEventListener('mousemove', onMouseMove)
  window.addEventListener('mouseup', onMouseUp)
  window.addEventListener('resize', onWinResize)
  document.addEventListener('argus-paper-drag-start', onPaperDragStart)
  document.addEventListener('argus-paper-drag-end', onPaperDragEnd)
  window.addEventListener('argus-switch-sidebar-tab', onSwitchSidebarTab)
  restoreWindowSize()

  unlistenOpenPaper = await listen<{ slug: string; title?: string }>('argus-open-paper', (event) => {
    const slug = event.payload?.slug
    if (!slug) return
    const paper = libraryStore.papers.find(p => p.slug === slug)
    selectionStore.selectPaper(slug)
    showCanvas.value = false
    readerStore.openPaper(slug, event.payload.title || paper?.title || slug)
    rightSidebarVisible.value = true
    if (!PAPER_TABS.includes(sidebarTab.value)) {
      sidebarTab.value = 'metadata'
    }
  })

  // Load settings and collections on startup.
  if (libraryStore.currentPath) {
    await settingsStore.load()
    await collectionsStore.load()
    await aiStore.load()
    await cliStore.load()
    readerStore.loadTabs(libraryStore.currentPath)
  }

  // Tauri 2 file drag-drop
  try {
    const webview = getCurrentWebviewWindow()
    unlistenDragDrop = await webview.onDragDropEvent((event) => {
      const payload = event.payload
      if (isPaperDragging.value) {
        isDragging.value = false
        return
      }

      if (payload.type === 'enter') {
        const pdfs = payload.paths.filter((p: string) => p.toLowerCase().endsWith('.pdf'))
        isDragging.value = pdfs.length > 0
      } else if (payload.type === 'over') {
        // Keep the current state from the enter event; over events don't include paths.
      } else if (payload.type === 'leave') {
        isDragging.value = false
      } else if (payload.type === 'drop') {
        isDragging.value = false
        if (!libraryStore.currentPath || !selectionStore.activeCollectionId) return
        const pdfs = payload.paths.filter((p: string) => p.toLowerCase().endsWith('.pdf'))
        if (pdfs.length > 0) {
          importStore.importFiles(pdfs, selectionStore.activeCollectionId)
        }
      }
    })
  } catch (e) {
    console.warn('Drag-drop listener not available:', e)
  }
})

onUnmounted(() => {
  window.removeEventListener('mousemove', onMouseMove)
  window.removeEventListener('mouseup', onMouseUp)
  window.removeEventListener('resize', onWinResize)
  if (winResizeTimer) clearTimeout(winResizeTimer)
  document.removeEventListener('argus-paper-drag-start', onPaperDragStart)
  document.removeEventListener('argus-paper-drag-end', onPaperDragEnd)
  window.removeEventListener('argus-switch-sidebar-tab', onSwitchSidebarTab)
  unlistenOpenPaper?.()
  unlistenDragDrop?.()
})

// ── Resizable columns ─────────────────────────────────────────────────────────
const leftWidth = ref(loadLayoutNumber(MAIN_LEFT_WIDTH_KEY, DEFAULT_LEFT_WIDTH, MIN_LEFT_WIDTH, MAX_LEFT_WIDTH))
const rightWidth = ref(loadLayoutNumber(MAIN_RIGHT_WIDTH_KEY, DEFAULT_RIGHT_WIDTH, MIN_RIGHT_WIDTH, MAX_RIGHT_WIDTH))

let dragging: 'left' | 'right' | null = null
const activeResizeSide = ref<'left' | 'right' | null>(null)
let startX = 0
let startWidth = 0

function startResize(side: 'left' | 'right', e: MouseEvent) {
  dragging = side
  activeResizeSide.value = side
  startX = e.clientX
  startWidth = side === 'left' ? leftWidth.value : rightWidth.value
  document.body.style.userSelect = 'none'
  document.body.style.cursor = 'col-resize'
}

function onMouseMove(e: MouseEvent) {
  if (!dragging) return
  const delta = e.clientX - startX
  if (dragging === 'left') {
    leftWidth.value = Math.max(MIN_LEFT_WIDTH, Math.min(MAX_LEFT_WIDTH, startWidth + delta))
  } else {
    rightWidth.value = Math.max(MIN_RIGHT_WIDTH, Math.min(MAX_RIGHT_WIDTH, startWidth - delta))
  }
}

function onMouseUp() {
  const finishedSide = dragging
  dragging = null
  activeResizeSide.value = null
  document.body.style.userSelect = ''
  document.body.style.cursor = ''
  if (finishedSide === 'left') {
    saveLayoutNumber(MAIN_LEFT_WIDTH_KEY, leftWidth.value)
  } else if (finishedSide === 'right') {
    saveLayoutNumber(MAIN_RIGHT_WIDTH_KEY, rightWidth.value)
  }
}

async function onLibraryOpened() {
  await settingsStore.load()
  await collectionsStore.load()
  await aiStore.load()
  await cliStore.load()
  readerStore.loadTabs(libraryStore.currentPath!)
}

// Load all stores when library path becomes available (handles auto-restore on startup).
watch(
  () => libraryStore.currentPath,
  async (newPath, oldPath) => {
    if (newPath && !oldPath) {
      await onLibraryOpened()
    }
  }
)

watch(rightSidebarVisible, (visible) => {
  try {
    localStorage.setItem(MAIN_RIGHT_VISIBLE_KEY, String(visible))
  } catch {}
})

watch(sidebarTab, (tab) => {
  if (!PAPER_TABS.includes(tab)) return
  try {
    localStorage.setItem(MAIN_RIGHT_TAB_KEY, tab)
  } catch {}
})

// Auto-save tabs whenever they change (length, order, or active)
watch(
  [() => [...readerStore.tabs], () => readerStore.activeSlug],
  () => {
    if (libraryStore.currentPath) {
      readerStore.saveTabs(libraryStore.currentPath)
    }
  },
  { deep: true }
)

// Sidebar navigation should always bring the main area back to the library list
// tab. The opened PDF tab stays open; it just stops being the active tab.
watch(
  () => selectionStore.navSelectionSeq,
  () => {
    if (readerStore.activeSlug) {
      readerStore.showList()
    }
    if (showCanvas.value) {
      showCanvas.value = false
    }
  }
)
</script>

<template>
  <!-- Startup check: keep blank until we know whether a saved library exists -->
  <div v-if="libraryStore.isCheckingLibrary" class="startup-blank">
    <div class="welcome-drag" data-tauri-drag-region />
  </div>

  <!-- Startup restore / opening library -->
  <div v-else-if="showLibraryLoading" class="library-loading">
    <div class="welcome-drag" data-tauri-drag-region />
    <div class="library-loading-card">
      <div class="library-spinner" aria-hidden="true">
        <span />
        <span />
        <span />
      </div>
      <div class="library-loading-title">{{ t('welcome.loading') }}</div>
      <div class="library-loading-sub">{{ t('welcome.loadingSub') }}</div>
    </div>
  </div>

  <!-- No library open → welcome screen -->
  <div v-else-if="!libraryStore.currentPath" class="welcome">
    <!-- Drag region replacing the native title bar -->
    <div class="welcome-drag" data-tauri-drag-region />
    <div class="welcome-card">
      <div class="welcome-icon">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
          <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
        </svg>
      </div>
      <h1>Argus</h1>
      <p>{{ t('welcome.tagline') }}</p>
      <button class="btn-primary" @click="libraryStore.pickAndOpen()">
        {{ t('welcome.open') }}
      </button>
      <p v-if="libraryStore.error" class="error-text">{{ libraryStore.error }}</p>
    </div>
  </div>

  <!-- Library loaded → 3-column layout -->
  <div v-else class="main-layout">
    <!-- Title bar: sits above everything, drag region + tabs next to traffic lights -->
    <TabBar />

    <Toolbar
      :left-sidebar-width="leftWidth"
      :right-sidebar-open="rightSidebarVisible"
      :right-sidebar-width="rightWidth"
      :sidebar-tab="sidebarTab"
      @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
      @update:sidebar-tab="sidebarTab = $event"
    />

    <div class="columns">
      <LeftSidebar
        v-model:show-settings="showSettings"
        :style="{ width: leftWidth + 'px', minWidth: leftWidth + 'px' }"
        @open-canvas="onOpenCanvas"
      />

      <div
        class="divider"
        :class="{ active: activeResizeSide === 'left' }"
        @mousedown.prevent="startResize('left', $event)"
        title="Drag to resize"
      />

      <div class="paper-list-col center-col">
        <PdfViewer
          v-if="readerStore.activeSlug"
          ref="pdfViewerRef"
          :key="readerStore.activeSlug"
          class="center-fill"
          :right-sidebar-open="rightSidebarVisible"
          @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
        />
        <CanvasPanel
          v-else-if="showCanvas"
          class="center-fill"
          @select-paper="onCanvasSelectPaper"
          @close="showCanvas = false"
        />
        <div v-else class="center-fill">
          <PaperList />
        </div>
      </div>

      <Transition name="right-panel">
        <div
          v-if="rightSidebarVisible"
          class="right-panel-wrap"
          :style="{ width: (rightWidth + 1) + 'px', minWidth: (rightWidth + 1) + 'px' }"
        >
          <div
            class="divider"
            :class="{ active: activeResizeSide === 'right' }"
            @mousedown.prevent="startResize('right', $event)"
            title="Drag to resize"
          />

          <RightSidebar
            class="right-sidebar-panel"
            :active-tab="sidebarTab"
            @update:active-tab="sidebarTab = $event"
            @open-settings="openSettingsOnAi"
          />
        </div>
      </Transition>
    </div>

    <!-- Drag-drop overlay -->
    <Transition name="fade">
      <div v-if="isDragging" class="drag-overlay">
        <div class="drag-card">
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>
            <polyline points="17 8 12 3 7 8"/>
            <line x1="12" y1="3" x2="12" y2="15"/>
          </svg>
          <p>{{ selectionStore.activeCollectionId ? t('import.dropHere') : t('import.dropSelectCollection') }}</p>
          <span>{{ selectionStore.activeCollectionId ? t('import.dropSub') : t('import.dropSelectCollectionSub') }}</span>
        </div>
      </div>
    </Transition>

    <!-- Unified settings modal -->
    <SettingsModal
      v-if="showSettings"
      :initial-section="settingsAiSection ? 'ai' : undefined"
      @close="showSettings = false; settingsAiSection = false"
    />
  </div>
</template>

<style scoped>
/* Startup blank */
.startup-blank {
  flex: 1;
  background: var(--bg-primary);
}

/* Startup loading */
.library-loading {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  background: var(--bg-secondary);
}

.library-loading-card {
  min-width: 260px;
  padding: 54px 36px;
  text-align: center;
  color: var(--text-primary);
}

.library-spinner {
  width: 52px;
  height: 52px;
  margin: 0 auto 22px;
  position: relative;
}

.library-spinner span {
  position: absolute;
  inset: 0;
  border-radius: 50%;
  border: 2px solid transparent;
  border-top-color: var(--accent);
  animation: library-spin 1.05s linear infinite;
}

.library-spinner span:nth-child(2) {
  inset: 7px;
  border-top-color: color-mix(in srgb, var(--accent) 58%, var(--text-tertiary));
  animation-duration: 1.45s;
  animation-direction: reverse;
}

.library-spinner span:nth-child(3) {
  inset: 15px;
  border-top-color: color-mix(in srgb, var(--accent) 34%, var(--text-tertiary));
  animation-duration: 1.85s;
}

.library-loading-title {
  font-size: 17px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 7px;
}

.library-loading-sub {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
}

@keyframes library-spin {
  to { transform: rotate(360deg); }
}

/* Welcome */
.welcome {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-start;
  background: var(--bg-secondary);
}

.welcome-drag {
  width: 100%;
  height: 38px;
  flex-shrink: 0;
  -webkit-app-region: drag;
}

.welcome-card {
  text-align: center;
  max-width: 340px;
  padding: 48px 36px;
  background: var(--bg-primary);
  border-radius: 20px;
  border: 1px solid var(--border-subtle);
  box-shadow: var(--shadow-md);
}

.welcome-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 76px;
  height: 76px;
  border-radius: 20px;
  background: var(--accent-light);
  color: var(--accent);
  margin-bottom: 22px;
}

.welcome-card h1 {
  font-size: 26px;
  font-weight: 700;
  letter-spacing: -0.02em;
  margin-bottom: 8px;
  color: var(--text-primary);
}

.welcome-card p {
  color: var(--text-secondary);
  font-size: var(--font-size-md);
  margin-bottom: 28px;
  line-height: 1.55;
}

.btn-primary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-pill);
  padding: 9px 22px;
  font-size: var(--font-size-md);
  font-weight: 500;
  transition: background 0.15s;
}
.btn-primary:hover { background: var(--accent-hover); }

.error-text {
  color: #e53e3e;
  font-size: var(--font-size-sm);
  margin-top: 14px;
}

/* Main layout */
.main-layout {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  position: relative;
}

.columns {
  flex: 1;
  display: flex;
  overflow: hidden;
}

.paper-list-col {
  flex: 1;
  min-width: 0;
}

.center-col {
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.center-fill {
  flex: 1;
  min-height: 0;
}

.right-panel-wrap {
  display: flex;
  flex-shrink: 0;
  min-width: 301px;
  overflow: hidden;
  will-change: width, min-width, opacity;
}

.right-sidebar-panel {
  flex: 1;
  min-width: 0;
}

.right-panel-enter-active,
.right-panel-leave-active {
  transition:
    width 0.18s ease,
    min-width 0.18s ease,
    opacity 0.14s ease;
}

.right-panel-enter-from,
.right-panel-leave-to {
  width: 0 !important;
  min-width: 0 !important;
  opacity: 0;
}

/* Slim, subtle resize handle */
.divider {
  width: 1px;
  flex-shrink: 0;
  background: transparent;
  cursor: col-resize;
  position: relative;
}
.divider::after {
  content: '';
  position: absolute;
  top: 0; bottom: 0;
  left: -3px; right: -3px;
}
.divider::before {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  left: 50%;
  width: 1px;
  transform: translateX(-50%);
  background: var(--border-subtle);
  transition: width 0.12s ease, background 0.12s ease, box-shadow 0.12s ease;
}
.divider:hover::before,
.divider.active::before {
  width: 2px;
  background: var(--accent);
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 12%, transparent);
}

/* Drag overlay */
.drag-overlay {
  position: absolute;
  inset: 8px;
  background: color-mix(in srgb, var(--accent) 6%, var(--bg-primary));
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
  border: 2px dashed var(--accent);
  border-radius: var(--radius-lg);
  pointer-events: none;
}

.drag-card {
  text-align: center;
  color: var(--accent);
}

.drag-card p {
  font-size: 18px;
  font-weight: 600;
  margin: 12px 0 6px;
}

.drag-card span {
  font-size: var(--font-size-sm);
  opacity: 0.7;
}

.fade-enter-active, .fade-leave-active { transition: opacity 0.15s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
</style>
