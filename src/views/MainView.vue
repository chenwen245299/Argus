<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted, nextTick, defineAsyncComponent } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { Window as TauriWindow } from '@tauri-apps/api/window'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useImportStore } from '../stores/import'
import { useSettingsStore } from '../stores/settings'
import { useCollectionsStore } from '../stores/collections'
import { useAiStore } from '../stores/ai'
import { useSelectionStore } from '../stores/selection'
import { useCanvasStore } from '../stores/canvas'
import { useRagStore } from '../stores/rag'
import { useActivityStore } from '../stores/activity'
import { switchToTranslationsTab, askAiText } from '../stores/translationHistory'
import { pendingSnippet, initSnippetStore, libraries as snippetLibraries } from '../stores/snippetLibrary'
import { lists as writingLists, activeListId as writingActiveListId } from '../stores/writing'
import { isEbookFileType } from '../types'
import Toolbar from '../components/Toolbar.vue'
import LeftSidebar from '../components/LeftSidebar.vue'
import PaperList from '../components/PaperList.vue'
import TabBar from '../components/TabBar.vue'
import RightSidebar from '../components/RightSidebar.vue'
import AddSnippetModal from '../components/AddSnippetModal.vue'
import UpdatePrompt from '../components/UpdatePrompt.vue'
import RelatedPapersPopover from '../components/RelatedPapersPopover.vue'
import CitationGraphModal from '../components/CitationGraphModal.vue'

// Conditionally-rendered heavyweights (pdfjs / vue-flow / settings panels) are
// code-split so the main window paints before any of them download.
const PdfViewer = defineAsyncComponent(() => import('../components/PdfViewer.vue'))
const EbookViewer = defineAsyncComponent(() => import('../components/EbookViewer.vue'))
const CanvasPanel = defineAsyncComponent(() => import('../components/CanvasPanel.vue'))
const SettingsModal = defineAsyncComponent(() => import('../components/SettingsModal.vue'))
const WelcomeOnboarding = defineAsyncComponent(() => import('../components/WelcomeOnboarding.vue'))
const SnippetLibraryView = defineAsyncComponent(() => import('../components/SnippetLibraryView.vue'))
const WritingView = defineAsyncComponent(() => import('../components/WritingView.vue'))

const { t } = useI18n()
const libraryStore = useLibraryStore()
const readerStore = useReaderStore()
const importStore = useImportStore()
const settingsStore = useSettingsStore()
const collectionsStore = useCollectionsStore()
const aiStore = useAiStore()
const selectionStore = useSelectionStore()
const canvasStore = useCanvasStore()
const ragStore = useRagStore()
const activityStore = useActivityStore()

// ── Window size persistence ────────────────────────────────────────────────────
const WIN_SIZE_KEY = 'argus:window:size'
const MIN_MAIN_WINDOW_W = 1400
const MIN_MAIN_WINDOW_H = 600
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
      Math.max(MIN_MAIN_WINDOW_W, width),
      Math.max(MIN_MAIN_WINDOW_H, height),
    ))
  } catch {}
}

const showSettings = ref(false)
const settingsAiSection = ref(false)  // true = open settings on AI Services tab
const settingsSection = ref<string | undefined>(undefined)  // explicit section to open on
const MAIN_LEFT_WIDTH_KEY = 'argus:layout:left-width'
const MAIN_RIGHT_WIDTH_KEY = 'argus:layout:right-width'
const MAIN_RIGHT_VISIBLE_KEY = 'argus:layout:right-visible'
const MAIN_RIGHT_TAB_KEY = 'argus:layout:right-tab'
const PAPER_TABS = ['notes', 'highlights', 'sections', 'ai', 'metadata']
// Tabs available while the canvas/graph is shown (翻译/批注 hidden, 绘图 added).
const CANVAS_TABS = ['draw', 'notes', 'ai', 'metadata']
const MIN_LEFT_WIDTH = 240
const MAX_LEFT_WIDTH = 360
const DEFAULT_LEFT_WIDTH = 220
const MIN_RIGHT_WIDTH = 350
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

interface MainLayoutState {
  leftWidth?: number
  rightWidth?: number
  rightVisible?: boolean
  rightTab?: string
}

let mainLayoutSaveChain: Promise<unknown> = Promise.resolve()
let isRestoringMainLayout = false

function readLegacyMainLayout(): MainLayoutState {
  return {
    leftWidth: loadLayoutNumber(MAIN_LEFT_WIDTH_KEY, DEFAULT_LEFT_WIDTH, MIN_LEFT_WIDTH, MAX_LEFT_WIDTH),
    rightWidth: loadLayoutNumber(MAIN_RIGHT_WIDTH_KEY, DEFAULT_RIGHT_WIDTH, MIN_RIGHT_WIDTH, MAX_RIGHT_WIDTH),
    rightVisible: loadLayoutBoolean(MAIN_RIGHT_VISIBLE_KEY, true),
    rightTab: loadSidebarTab(),
  }
}

function normalizeMainLayout(input: unknown): MainLayoutState | null {
  if (!input || typeof input !== 'object') return null
  const raw = input as MainLayoutState
  return {
    leftWidth: Number.isFinite(raw.leftWidth) ? Math.min(MAX_LEFT_WIDTH, Math.max(MIN_LEFT_WIDTH, Number(raw.leftWidth))) : undefined,
    rightWidth: Number.isFinite(raw.rightWidth) ? Math.min(MAX_RIGHT_WIDTH, Math.max(MIN_RIGHT_WIDTH, Number(raw.rightWidth))) : undefined,
    rightVisible: typeof raw.rightVisible === 'boolean' ? raw.rightVisible : undefined,
    rightTab: raw.rightTab && PAPER_TABS.includes(raw.rightTab) ? raw.rightTab : undefined,
  }
}

function applyMainLayout(layout: MainLayoutState | null) {
  if (!layout) return
  isRestoringMainLayout = true
  if (layout.leftWidth) leftWidth.value = layout.leftWidth
  if (layout.rightWidth) rightWidth.value = layout.rightWidth
  if (typeof layout.rightVisible === 'boolean') rightSidebarVisible.value = layout.rightVisible
  if (layout.rightTab) sidebarTab.value = layout.rightTab
  nextTick(() => { isRestoringMainLayout = false })
}

async function loadMainLayout(path: string) {
  try {
    const uiState = await invoke<Record<string, unknown>>('get_library_ui_state', { root: path })
    const fromFile = normalizeMainLayout(uiState.mainLayout)
    if (fromFile) {
      applyMainLayout(fromFile)
      return
    }
  } catch (e) {
    console.error('[main] load ui_state layout failed:', e)
  }
  applyMainLayout(readLegacyMainLayout())
  saveMainLayout(path)
}

function saveMainLayout(path = libraryStore.currentPath) {
  if (!path || isRestoringMainLayout) return
  const layout: MainLayoutState = {
    leftWidth: Math.round(leftWidth.value),
    rightWidth: Math.round(rightWidth.value),
    rightVisible: rightSidebarVisible.value,
    rightTab: PAPER_TABS.includes(sidebarTab.value) ? sidebarTab.value : 'metadata',
  }
  mainLayoutSaveChain = mainLayoutSaveChain
    .catch(() => undefined)
    .then(() => invoke('patch_library_ui_state', {
      root: path,
      patch: { version: 1, mainLayout: layout },
    }))
    .catch(e => console.error('[main] save ui_state layout failed:', e))
}

const rightSidebarVisible = ref(loadLayoutBoolean(MAIN_RIGHT_VISIBLE_KEY, true))
const sidebarTab = ref<string>(loadSidebarTab())
// Slugs whose viewer has been created ("materialized"). A tab is materialized
// the first time it becomes active, then kept mounted while it stays open — so
// restored-but-unvisited tabs don't eagerly load on startup, and closed tabs
// drop out (their viewer unmounts, releasing the PDF).
const materializedSlugs = ref<Set<string>>(new Set())
watch(() => readerStore.activeSlug, (slug) => {
  if (slug && !materializedSlugs.value.has(slug)) {
    materializedSlugs.value = new Set(materializedSlugs.value).add(slug)
  }
}, { immediate: true })
const liveViewerSlugs = computed(() =>
  readerStore.tabs.filter(t => materializedSlugs.value.has(t.slug)).map(t => t.slug))

// Viewer routing: PDFs keep the untouched PdfViewer; ebooks get their own
// viewer. Tab.fileType is set at open time; tabs persisted before that field
// existed fall back to the library index entry.
function fileTypeFor(slug: string): string {
  const tab = readerStore.tabs.find(t => t.slug === slug)
  return tab?.fileType
    ?? libraryStore.papers.find(p => p.slug === slug)?.file_type
    ?? 'pdf'
}
function activePaperInfo() {
  const slug = readerStore.activeSlug
  if (!slug) return null
  const paper = libraryStore.papers.find(p => p.slug === slug)
  const tab = readerStore.tabs.find(t => t.slug === slug)
  return {
    slug,
    title: paper?.title ?? tab?.title ?? slug,
    fileType: paper?.file_type ?? tab?.fileType ?? 'pdf',
  }
}
const livePdfSlugs = computed(() => liveViewerSlugs.value.filter(s => !isEbookFileType(fileTypeFor(s))))
const liveEbookSlugs = computed(() => liveViewerSlugs.value.filter(s => isEbookFileType(fileTypeFor(s))))

const showCanvas = ref(false)
const showSnippetLibrary = ref(false)
const showWriting = ref(false)
// Open writing tabs (one per reference list; null = the "Library"/all-papers
// view). Each stays open for quick switching until its tab is closed, mirroring
// the snippet-library tabs. Names are derived reactively from the writing store.
const openWritingIds = ref<(string | null)[]>([])
const openWritingTabs = computed(() => {
  const out: { id: string; name: string }[] = []
  for (const id of openWritingIds.value) {
    if (id === null) continue   // the all-papers ("Library") view has no tab
    const list = writingLists.value.find(l => l.id === id)
    if (list) out.push({ id, name: list.name })
  }
  return out
})
const activeSnippetLibraryId = ref<string | null>(null)
const openSnippetLibraryIds = ref<string[]>([])
type WorkspaceMode = 'library' | 'canvas' | 'snippets' | 'writing'

// The workspace the center is currently showing. Drives the left sidebar so it
// always matches the active tab (open a paper → library, writing tab → writing…).
const activeWorkspace = computed<WorkspaceMode>(() => {
  if (readerStore.activeSlug) return 'library'   // a paper tab is open
  if (showWriting.value) return 'writing'
  if (showCanvas.value) return 'canvas'
  if (showSnippetLibrary.value) return 'snippets'
  return 'library'
})

const openSnippetLibraryTabs = computed(() =>
  openSnippetLibraryIds.value
    .map(id => snippetLibraries.value.find(lib => lib.id === id))
    .filter((lib): lib is NonNullable<typeof lib> => Boolean(lib))
)

// Sync showCanvas with canvasStore.isShown so TabBar close button works
watch(() => canvasStore.isShown, (v) => { showCanvas.value = v })

let autoOpeningCanvasId: string | null = null
async function ensureCanvasSelection() {
  if (!showCanvas.value || canvasStore.loading) return
  const currentId = canvasStore.currentCanvas?.id
  if (currentId && canvasStore.canvasList.some(canvas => canvas.id === currentId)) return
  const first = canvasStore.canvasList[0]
  if (!first || autoOpeningCanvasId === first.id) return
  autoOpeningCanvasId = first.id
  try {
    await canvasStore.openCanvas(first.id)
  } catch (e) {
    console.error('Auto-open canvas:', e)
  } finally {
    autoOpeningCanvasId = null
  }
}

watch(
  [() => showCanvas.value, () => canvasStore.canvasList.map(canvas => canvas.id).join('|')],
  () => { void ensureCanvasSelection() }
)

watch(
  [() => showSnippetLibrary.value, () => snippetLibraries.value.map(lib => lib.id).join('|')],
  ([visible]) => {
    if (!visible) return
    const currentId = activeSnippetLibraryId.value
    const nextId = currentId && snippetLibraries.value.some(lib => lib.id === currentId)
      ? currentId
      : (snippetLibraries.value[0]?.id ?? null)
    activeSnippetLibraryId.value = nextId
    if (nextId) rememberSnippetLibraryTab(nextId)
  }
)

const showLibraryLoading = computed(() =>
  libraryStore.isRestoringLibrary || (!libraryStore.currentPath && libraryStore.isLoading)
)

function rememberWritingTab(id: string | null) {
  if (id === null) return   // the all-papers ("Library") view is tab-less
  if (!openWritingIds.value.some(x => x === id)) {
    openWritingIds.value = [...openWritingIds.value, id]
  }
}

// Show one writing view (a specific list, or null = all papers) and give it a tab.
function activateWriting(id: string | null) {
  readerStore.showList()   // clear activeSlug so the reference table shows
  showCanvas.value = false
  canvasStore.isShown = false
  showSnippetLibrary.value = false
  writingActiveListId.value = id
  rememberWritingTab(id)
  showWriting.value = true
}

// Entering the writing workspace from the dropdown re-opens the last-active view
// (or the all-papers view the first time).
function onOpenWriting() {
  activateWriting(writingActiveListId.value ?? null)
}

// Opening a specific folder/list from the sidebar (payload carries its id; null
// = the "Library" all-papers view).
function onOpenWritingList(id: string | null) {
  activateWriting(id)
}

function closeWritingTab(id: string | null) {
  const idx = openWritingIds.value.findIndex(x => x === id)
  if (idx === -1) return
  const next = openWritingIds.value.filter(x => x !== id)
  openWritingIds.value = next

  if (writingActiveListId.value === id && showWriting.value) {
    const fallback = next[Math.min(idx, next.length - 1)]
    if (fallback !== undefined) activateWriting(fallback)
    else showWriting.value = false
  }
}

// Prune tabs for lists deleted elsewhere; fall back to all-papers if the active
// list is gone.
watch(
  () => writingLists.value.map(l => l.id).join('|'),
  () => {
    const valid = new Set(writingLists.value.map(l => l.id))
    openWritingIds.value = openWritingIds.value.filter(id => id === null || valid.has(id))
    if (showWriting.value && writingActiveListId.value !== null && !valid.has(writingActiveListId.value)) {
      activateWriting(null)
    }
  }
)

function onOpenLibrary() {
  readerStore.showList()
  showCanvas.value = false
  canvasStore.isShown = false
  showSnippetLibrary.value = false
  showWriting.value = false
}

function onOpenCanvas() {
  readerStore.showList()   // clear activeSlug so PdfViewer v-if yields to CanvasPanel
  showCanvas.value = true
  canvasStore.isShown = true
  showSnippetLibrary.value = false
  showWriting.value = false
  // Land on the drawing panel (and leave the PDF-only tabs behind).
  if (!CANVAS_TABS.includes(sidebarTab.value)) sidebarTab.value = 'draw'
}

function closeCanvas() {
  showCanvas.value = false
  canvasStore.isShown = false
  // 'draw' only exists in canvas mode — fall back to a paper tab.
  if (!PAPER_TABS.includes(sidebarTab.value)) sidebarTab.value = 'metadata'
}

function rememberSnippetLibraryTab(libraryId: string) {
  if (!openSnippetLibraryIds.value.includes(libraryId)) {
    openSnippetLibraryIds.value = [...openSnippetLibraryIds.value, libraryId]
  }
}

function activateSnippetLibrary(libraryId: string) {
  readerStore.showList()
  showCanvas.value = false
  canvasStore.isShown = false
  showWriting.value = false
  rememberSnippetLibraryTab(libraryId)
  activeSnippetLibraryId.value = libraryId
  showSnippetLibrary.value = true
}

function onOpenSnippetWorkspace() {
  readerStore.showList()
  showCanvas.value = false
  canvasStore.isShown = false
  showWriting.value = false
  showSnippetLibrary.value = true

  const currentId = activeSnippetLibraryId.value
  const nextId = currentId && snippetLibraries.value.some(lib => lib.id === currentId)
    ? currentId
    : (snippetLibraries.value[0]?.id ?? null)
  activeSnippetLibraryId.value = nextId
  if (nextId) rememberSnippetLibraryTab(nextId)
}

function onSwitchWorkspace(mode: WorkspaceMode) {
  if (mode === 'library') {
    onOpenLibrary()
  } else if (mode === 'canvas') {
    onOpenCanvas()
    void ensureCanvasSelection()
  } else if (mode === 'snippets') {
    onOpenSnippetWorkspace()
  } else {
    onOpenWriting()
  }
}

function onOpenSnippetLibrary(libraryId: string) {
  activateSnippetLibrary(libraryId)
}

function hideSnippetLibrary() {
  showSnippetLibrary.value = false
  // Clicking the home/canvas tab should also leave the writing view.
  showWriting.value = false
}

function closeSnippetLibraryTab(libraryId: string) {
  const idx = openSnippetLibraryIds.value.indexOf(libraryId)
  if (idx === -1) return
  const nextIds = openSnippetLibraryIds.value.filter(id => id !== libraryId)
  openSnippetLibraryIds.value = nextIds

  if (activeSnippetLibraryId.value !== libraryId) return

  const nextId = nextIds[Math.min(idx, nextIds.length - 1)]
  if (nextId) {
    activateSnippetLibrary(nextId)
  } else {
    showSnippetLibrary.value = false
    activeSnippetLibraryId.value = null
  }
}

function onSnippetOpenPaper(slug: string, page: number, title: string) {
  showSnippetLibrary.value = false
  readerStore.openPaper(slug, title, libraryStore.papers.find(p => p.slug === slug)?.file_type)
  readerStore.pendingPageJump = page
}

function onWritingSelectPaper(slug: string) {
  selectionStore.selectPaper(slug)
  rightSidebarVisible.value = true
  // Land on a paper tab so the right sidebar shows notes/metadata for the pick.
  if (!PAPER_TABS.includes(sidebarTab.value)) sidebarTab.value = 'notes'
}

function onCanvasSelectPaper(slug: string) {
  selectionStore.selectPaper(slug)
  rightSidebarVisible.value = true
  // In canvas mode, clicking any element jumps to the drawing properties tab.
  if (showCanvas.value) {
    sidebarTab.value = 'draw'
    return
  }
  if (!PAPER_TABS.includes(sidebarTab.value)) {
    sidebarTab.value = 'metadata'
  }
}

// Selecting any element on the canvas (node / text / shape / line) jumps the
// sidebar to the drawing properties tab so its properties are visible.
watch(() => canvasStore.selectedNodeIds.length, (n) => {
  if (showCanvas.value && n > 0) {
    sidebarTab.value = 'draw'
    rightSidebarVisible.value = true
  }
})

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

function openSettingsSection(section?: 'rag') {
  settingsSection.value = section
  showSettings.value = true
}

// ── First-run onboarding ────────────────────────────────────────────────────────
// Guides the user through configuring the two things needed for full
// functionality: a default AI model and a Semantic Scholar key. Shown on every
// launch while setup is still incomplete, until the user opts out via the
// "don't remind me again" checkbox (which sets the flag below).
const ONBOARDING_DISMISSED_KEY = 'argus:onboarding:dismissed'
const showOnboarding = ref(false)

function onboardingNeeded(): boolean {
  const modelDone = !!aiStore.findModel(aiStore.defaultSelection)
  return !modelDone || !settingsStore.semanticScholarConfigured
}

// Call after the per-library stores have loaded. Show every launch while setup
// is incomplete, unless the user has permanently dismissed the guide.
function maybeShowOnboarding() {
  if (localStorage.getItem(ONBOARDING_DISMISSED_KEY)) return
  if (onboardingNeeded()) showOnboarding.value = true
}

// `permanent` is true when the user ticked "don't remind me again" (or finished
// once everything is configured) — only then do we stop showing it on launch.
function dismissOnboarding(permanent: boolean) {
  showOnboarding.value = false
  if (permanent) {
    try { localStorage.setItem(ONBOARDING_DISMISSED_KEY, '1') } catch {}
  }
}

// Manual re-trigger hook (e.g. a "review setup guide" entry): ignores the flag.
function onShowOnboardingEvent() { showOnboarding.value = true }

// Open the settings modal to a given section from anywhere (window event).
function onOpenSettingsEvent(event: Event) {
  const { section } = (event as CustomEvent<{ section?: string }>).detail ?? {}
  settingsSection.value = section
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

// ── Activity tracking ─────────────────────────────────────────────────────────
let activityStatusReady = false
const knownReadingStatus = new Map<string, string>()
const knownAiSummaryDone = new Map<string, boolean>()

function syncActivityReading() {
  const info = activePaperInfo()
  const canTrack =
    !!libraryStore.currentPath &&
    !!info &&
    document.visibilityState !== 'hidden' &&
    document.hasFocus()

  if (canTrack && info) {
    activityStore.beginReading(info.slug, info.title, info.fileType)
  } else {
    activityStore.endReading()
  }
}

function onActivityFocusChange() {
  syncActivityReading()
}

function onActivityUnload() {
  activityStore.endReading()
}

async function syncActivityLibrary(path: string | null) {
  await activityStore.load(path)
  knownReadingStatus.clear()
  knownAiSummaryDone.clear()
  libraryStore.papers.forEach(paper => {
    knownReadingStatus.set(paper.slug, paper.reading_status)
    knownAiSummaryDone.set(paper.slug, paper.status.ai_summary_done)
  })
  activityStatusReady = true
  syncActivityReading()
}

// ── Drag-drop ──────────────────────────────────────────────────────────────────
const isDragging = ref(false)
const isPaperDragging = ref(false)
const canImportIntoActiveCollection = computed(() =>
  collectionsStore.canReceivePapers(selectionStore.activeCollectionId)
)
const dragDropTitle = computed(() => {
  if (canImportIntoActiveCollection.value) return t('import.dropHere')
  return selectionStore.activeCollectionId
    ? t('import.dropSelectSubCollection')
    : t('import.dropSelectCollection')
})
const dragDropSubtitle = computed(() => {
  if (canImportIntoActiveCollection.value) return t('import.dropSub')
  return selectionStore.activeCollectionId
    ? t('import.dropSelectSubCollectionSub')
    : t('import.dropSelectCollectionSub')
})
let unlistenDragDrop: (() => void) | null = null
let unlistenOpenPaper: UnlistenFn | null = null
let unlistenLibraryPaperAdded: UnlistenFn | null = null
let mainFocusRetryTimer: number | null = null

async function focusMainWindowNow() {
  const mainWindow = TauriWindow.getCurrent()
  await mainWindow.show().catch(() => {})
  await mainWindow.unminimize().catch(() => {})
  await mainWindow.setFocus().catch(() => {})
}

function scheduleMainWindowFocus() {
  if (mainFocusRetryTimer) clearTimeout(mainFocusRetryTimer)

  void nextTick(async () => {
    await focusMainWindowNow()
    mainFocusRetryTimer = window.setTimeout(() => {
      focusMainWindowNow().catch(() => {})
      mainFocusRetryTimer = null
    }, 180)
  })
}

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
  window.addEventListener('focus', onActivityFocusChange)
  window.addEventListener('blur', onActivityFocusChange)
  window.addEventListener('beforeunload', onActivityUnload)
  document.addEventListener('visibilitychange', onActivityFocusChange)
  document.addEventListener('argus-paper-drag-start', onPaperDragStart)
  document.addEventListener('argus-paper-drag-end', onPaperDragEnd)
  window.addEventListener('argus-switch-sidebar-tab', onSwitchSidebarTab)
  window.addEventListener('argus-open-settings', onOpenSettingsEvent)
  window.addEventListener('argus-show-onboarding', onShowOnboardingEvent)
  activityStore.startHeartbeat()
  await syncActivityLibrary(libraryStore.currentPath)
  restoreWindowSize()

  unlistenLibraryPaperAdded = await listen<{ slug?: string; title?: string }>('library-paper-added', async (event) => {
    await Promise.all([libraryStore.refresh(), collectionsStore.load()])
    // arXiv-added papers arrive with only their arXiv source metadata; run the
    // same full AI-metadata + Semantic Scholar / easyScholar pipeline as a local
    // import so they end up with equivalent metadata coverage.
    const slug = event.payload?.slug
    if (slug) importStore.processAddedPaper(slug, event.payload?.title)
  })

  unlistenOpenPaper = await listen<{ slug: string; title?: string }>('argus-open-paper', (event) => {
    const slug = event.payload?.slug
    if (!slug) return
    const paper = libraryStore.papers.find(p => p.slug === slug)
    selectionStore.selectPaper(slug)
    showCanvas.value = false
    canvasStore.isShown = false
    readerStore.openPaper(slug, event.payload.title || paper?.title || slug, paper?.file_type)
    rightSidebarVisible.value = true
    if (!PAPER_TABS.includes(sidebarTab.value)) {
      sidebarTab.value = 'metadata'
    }
    scheduleMainWindowFocus()
  })

  // Load settings and collections on startup (independent — load in parallel).
  if (libraryStore.currentPath) {
    await Promise.all([settingsStore.load(), collectionsStore.load(), aiStore.load(), ragStore.load()])
    await readerStore.loadTabs(libraryStore.currentPath)
    await loadMainLayout(libraryStore.currentPath)
    initSnippetStore()
    maybeShowOnboarding()
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

      const IMPORTABLE_RE = /\.(pdf|epub|mobi|azw3|azw|fb2|txt|zip)$/i
      if (payload.type === 'enter') {
        const docs = payload.paths.filter((p: string) => IMPORTABLE_RE.test(p))
        isDragging.value = docs.length > 0
      } else if (payload.type === 'over') {
        // Keep the current state from the enter event; over events don't include paths.
      } else if (payload.type === 'leave') {
        isDragging.value = false
      } else if (payload.type === 'drop') {
        isDragging.value = false
        if (!libraryStore.currentPath || !selectionStore.activeCollectionId || !canImportIntoActiveCollection.value) return
        const docs = payload.paths.filter((p: string) => IMPORTABLE_RE.test(p))
        if (docs.length > 0) {
          importStore.importFiles(docs, selectionStore.activeCollectionId)
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
  window.removeEventListener('focus', onActivityFocusChange)
  window.removeEventListener('blur', onActivityFocusChange)
  window.removeEventListener('beforeunload', onActivityUnload)
  document.removeEventListener('visibilitychange', onActivityFocusChange)
  activityStore.stopHeartbeat()
  if (winResizeTimer) clearTimeout(winResizeTimer)
  if (mainFocusRetryTimer) clearTimeout(mainFocusRetryTimer)
  document.removeEventListener('argus-paper-drag-start', onPaperDragStart)
  document.removeEventListener('argus-paper-drag-end', onPaperDragEnd)
  window.removeEventListener('argus-switch-sidebar-tab', onSwitchSidebarTab)
  window.removeEventListener('argus-open-settings', onOpenSettingsEvent)
  window.removeEventListener('argus-show-onboarding', onShowOnboardingEvent)
  unlistenOpenPaper?.()
  unlistenDragDrop?.()
  unlistenLibraryPaperAdded?.()
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
  saveMainLayout()
}

async function onLibraryOpened() {
  await Promise.all([settingsStore.load(), collectionsStore.load(), aiStore.load(), ragStore.load()])
  await readerStore.loadTabs(libraryStore.currentPath!)
  await loadMainLayout(libraryStore.currentPath!)
  initSnippetStore()
  maybeShowOnboarding()
}

// Reload per-library stores whenever the library path changes — both the
// startup auto-restore (null → path) and an in-app library switch (A → B).
// Without the switch case, the sidebar kept the previous library's
// collections/settings/AI/RAG state.
watch(
  () => libraryStore.currentPath,
  async (newPath, oldPath) => {
    if (newPath && newPath !== oldPath) {
      await onLibraryOpened()
    }
    if (newPath !== oldPath) await syncActivityLibrary(newPath)
  }
)

watch(
  () => readerStore.activeSlug,
  () => {
    const info = activePaperInfo()
    if (info && libraryStore.currentPath) {
      activityStore.recordOpenPaper(info.slug, info.title, info.fileType)
    }
    syncActivityReading()
  },
  { immediate: true }
)

watch(
  () => libraryStore.papers.map(p => ({
    slug: p.slug,
    title: p.title,
    fileType: p.file_type,
    readingStatus: p.reading_status,
    aiDone: p.status.ai_summary_done,
  })),
  (items) => {
    if (!activityStatusReady) {
      items.forEach(item => {
        knownReadingStatus.set(item.slug, item.readingStatus)
        knownAiSummaryDone.set(item.slug, item.aiDone)
      })
      activityStatusReady = true
      return
    }

    for (const item of items) {
      const prevReading = knownReadingStatus.get(item.slug)
      const prevAi = knownAiSummaryDone.get(item.slug)
      if (prevReading !== undefined && prevReading !== 'read' && item.readingStatus === 'read') {
        activityStore.recordCompletedPaper(item.slug, item.title, item.fileType)
      }
      if (prevAi !== undefined && !prevAi && item.aiDone) {
        activityStore.recordAiAnalysis(item.slug, item.title, item.fileType)
      }
      knownReadingStatus.set(item.slug, item.readingStatus)
      knownAiSummaryDone.set(item.slug, item.aiDone)
    }
  },
  { deep: true, immediate: true }
)

watch(rightSidebarVisible, (visible) => {
  try {
    localStorage.setItem(MAIN_RIGHT_VISIBLE_KEY, String(visible))
  } catch {}
  saveMainLayout()
})

watch(sidebarTab, (tab) => {
  if (!PAPER_TABS.includes(tab)) return
  try {
    localStorage.setItem(MAIN_RIGHT_TAB_KEY, tab)
  } catch {}
  saveMainLayout()
})

watch(switchToTranslationsTab, (val) => {
  if (!val) return
  switchToTranslationsTab.value = false
  sidebarTab.value = 'translations'
  if (!rightSidebarVisible.value) rightSidebarVisible.value = true
})

watch(askAiText, (val) => {
  if (val === null) return
  sidebarTab.value = 'ai'
  if (!rightSidebarVisible.value) rightSidebarVisible.value = true
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

// Switching to a paper tab should close the snippet library panel
watch(
  () => readerStore.activeSlug,
  (slug) => {
    if (slug) {
      showSnippetLibrary.value = false
      showWriting.value = false
    }
  }
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
      canvasStore.isShown = false
    }
    if (showSnippetLibrary.value) {
      showSnippetLibrary.value = false
    }
    if (showWriting.value) {
      showWriting.value = false
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
        <Icon icon="fluent:book-24-regular" width="48" height="48" />
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
    <TabBar
      :right-sidebar-open="rightSidebarVisible"
      :snippet-library-tabs="openSnippetLibraryTabs"
      :snippet-library-visible="showSnippetLibrary"
      :active-snippet-library-id="activeSnippetLibraryId"
      :writing-tabs="openWritingTabs"
      :writing-visible="showWriting"
      :active-writing-id="writingActiveListId"
      @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
      @show-home="hideSnippetLibrary"
      @show-canvas="hideSnippetLibrary"
      @switch-snippet-library="activateSnippetLibrary"
      @close-snippet-library-tab="closeSnippetLibraryTab"
      @switch-writing="onOpenWritingList"
      @close-writing-tab="closeWritingTab"
    />

    <Toolbar
      :left-sidebar-width="leftWidth"
      :right-sidebar-open="rightSidebarVisible"
      :right-sidebar-width="rightWidth"
      :sidebar-tab="sidebarTab"
      :canvas-mode="showCanvas"
      @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
      @update:sidebar-tab="sidebarTab = $event"
    />

    <div class="columns">
      <LeftSidebar
        v-model:show-settings="showSettings"
        :snippet-library-visible="showSnippetLibrary"
        :active-snippet-library-id="activeSnippetLibraryId"
        :active-workspace="activeWorkspace"
        :style="{ width: leftWidth + 'px', minWidth: leftWidth + 'px' }"
        @switch-workspace="onSwitchWorkspace"
        @open-canvas="onOpenCanvas"
        @open-snippet-library="onOpenSnippetLibrary"
        @open-writing="onOpenWritingList"
      />

      <div
        class="divider"
        :class="{ active: activeResizeSide === 'left' }"
        @mousedown.prevent="startResize('left', $event)"
        title="Drag to resize"
      />

      <div class="paper-list-col center-col">
        <!-- One live viewer per open tab, created lazily on first view and kept
             mounted while the tab stays open — so switching between tabs is
             instant (no re-fetch / re-parse). Only the active tab is shown.
             Closing a tab removes it from this list, unmounting its viewer and
             fully releasing the PDF (see PdfViewer's onUnmounted). -->
        <PdfViewer
          v-for="s in livePdfSlugs"
          v-show="s === readerStore.activeSlug"
          :key="s"
          :slug="s"
          class="center-fill"
          :right-sidebar-open="rightSidebarVisible"
          @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
        />
        <EbookViewer
          v-for="s in liveEbookSlugs"
          v-show="s === readerStore.activeSlug"
          :key="`ebook:${s}`"
          :slug="s"
          class="center-fill"
          :right-sidebar-open="rightSidebarVisible"
          @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
        />
        <!-- Non-PDF center views (shown only when no PDF tab is active) -->
        <template v-if="!readerStore.activeSlug">
          <CanvasPanel
            v-if="showCanvas"
            class="center-fill"
            @select-paper="onCanvasSelectPaper"
            @close="closeCanvas()"
          />
          <SnippetLibraryView
            v-else-if="showSnippetLibrary && activeSnippetLibraryId"
            :library-id="activeSnippetLibraryId"
            class="center-fill"
            @open-paper="onSnippetOpenPaper"
            @open-settings="openSettingsSection"
          />
          <div v-else-if="showSnippetLibrary" class="center-fill workspace-empty">
            <Icon icon="fluent:folder-24-regular" width="44" height="44" />
            <p>{{ t('snippets.noLibraries') }}</p>
          </div>
          <WritingView
            v-else-if="showWriting"
            class="center-fill"
            @select-paper="onWritingSelectPaper"
          />
          <div v-else class="center-fill">
            <PaperList />
          </div>
        </template>
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
          <Icon icon="fluent:arrow-upload-24-regular" width="40" height="40" />
          <p>{{ dragDropTitle }}</p>
          <span>{{ dragDropSubtitle }}</span>
        </div>
      </div>
    </Transition>

    <!-- First-run onboarding (guides default-model + Semantic Scholar setup) -->
    <WelcomeOnboarding v-if="showOnboarding" @close="dismissOnboarding" />

    <!-- Unified settings modal -->
    <SettingsModal
      v-if="showSettings"
      :initial-section="settingsSection ?? (settingsAiSection ? 'ai' : undefined)"
      @close="showSettings = false; settingsAiSection = false; settingsSection = undefined"
    />

    <!-- Add to Snippet Library modal -->
    <AddSnippetModal
      v-if="pendingSnippet"
      :pending="pendingSnippet"
      @close="pendingSnippet = null"
    />

    <!-- Global update-available prompt (auto checks fire even when settings is closed) -->
    <UpdatePrompt />

    <!-- Related-papers popover (opened from viewer toolbars and the list right-click menu) -->
    <RelatedPapersPopover />

    <!-- Citation graph modal (opened from the viewer toolbar) -->
    <CitationGraphModal />
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

.workspace-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
  background: var(--bg-primary);
}
.workspace-empty p { margin: 0; font-size: var(--font-size-sm); }

.right-panel-wrap {
  display: flex;
  flex-shrink: 0;
  min-width: 301px;
  overflow: hidden;
  will-change: width, min-width, opacity;
  align-self: stretch;
  height: 100%;
}

.right-sidebar-panel {
  flex: 1;
  min-width: 0;
  height: 100%;
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
  background: var(--border-default);
  cursor: col-resize;
  position: relative;
  z-index: 2;
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
  background: transparent;
  transition: width 0.12s ease, background 0.12s ease, box-shadow 0.12s ease;
}
.divider:hover::before,
.divider.active::before {
  width: 3px;
  background: var(--accent);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 12%, transparent);
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
