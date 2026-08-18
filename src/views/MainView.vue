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
import { useReaderStore, featureTabKey } from '../stores/reader'
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
// Tabs available while the canvas/graph is shown (翻译/批注 hidden, 绘图 and the
// canvas chat added).
const CANVAS_TABS = ['draw', 'canvasChat', 'notes', 'ai', 'metadata']
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

// Which workspace the centre shows is decided entirely by the active tab, so
// these are derived, never assigned. They used to be independent refs that every
// navigation path had to reset in concert; one active tab makes them mutually
// exclusive by construction.
const activeTabKind = computed(() => {
  const tab = readerStore.activeTab
  return tab ? (tab.kind ?? 'paper') : null
})
const showCanvas = computed(() => activeTabKind.value === 'canvas')
const showSnippetLibrary = computed(() => activeTabKind.value === 'snippets')
const showWriting = computed(() => activeTabKind.value === 'writing')

/** The open canvas ids, in tab order. */
const openCanvasIds = computed(() =>
  readerStore.featureTabs('canvas')
    .map(t => t.refId)
    .filter((id): id is string => !!id))

// Canvas panels are instantiated on first visit and then kept alive, mirroring
// `materializedSlugs` for papers: a restored-but-unvisited canvas tab doesn't
// load its document on startup, and a closed one unmounts.
const materializedCanvasIds = ref<Set<string>>(new Set())
const liveCanvasIds = computed(() =>
  openCanvasIds.value.filter(id => materializedCanvasIds.value.has(id)))
const activeCanvasId = computed(() =>
  showCanvas.value ? readerStore.activeTab?.refId ?? null : null)
const activeSnippetLibraryId = computed(() =>
  showSnippetLibrary.value ? readerStore.activeTab?.refId ?? null : null)
const activeWritingId = computed(() =>
  showWriting.value ? readerStore.activeTab?.refId ?? null : null)

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

// The writing workspace's all-papers ("Library") view has no list id, but every
// workspace on screen now needs a tab to be reachable — so it gets a reserved one.
const WRITING_ALL = '__all__'

// The active tab is the source of truth; these keep the stores that still track
// their own "current" pointer (canvas documents, the writing list) in step.
watch(activeCanvasId, (id) => {
  canvasStore.isShown = !!id
  if (!id) return
  if (!materializedCanvasIds.value.has(id)) {
    materializedCanvasIds.value = new Set(materializedCanvasIds.value).add(id)
  }
  canvasStore.openCanvas(id).catch(e => console.error('Open canvas:', e))
}, { immediate: true })

// Free the document of a canvas whose tab was closed (flushing pending edits).
watch(
  () => openCanvasIds.value.join('|'),
  () => {
    const open = new Set(openCanvasIds.value)
    const closed = [...materializedCanvasIds.value].filter(id => !open.has(id))
    if (!closed.length) return
    materializedCanvasIds.value = new Set(
      [...materializedCanvasIds.value].filter(id => open.has(id)))
    for (const id of closed) void canvasStore.closeCanvas(id)
  }
)

watch(activeWritingId, (id) => {
  if (id !== null) writingActiveListId.value = id === WRITING_ALL ? null : id
}, { immediate: true })

const showLibraryLoading = computed(() =>
  libraryStore.isRestoringLibrary || (!libraryStore.currentPath && libraryStore.isLoading)
)

/** Open (or focus) a canvas tab. Several canvases can be open at once. */
function activateCanvas(id: string) {
  const name = canvasStore.canvasList.find(c => c.id === id)?.name ?? t('canvas.untitled')
  readerStore.openFeatureTab('canvas', id, name)
}

// Show one writing view (a specific list, or null = all papers) and give it a tab.
function activateWriting(id: string | null) {
  const refId = id ?? WRITING_ALL
  const name = id === null
    ? t('writing.allPapers')
    : writingLists.value.find(l => l.id === id)?.name ?? t('writing.title')
  readerStore.openFeatureTab('writing', refId, name)
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

/**
 * Keep one kind of feature tab in step with the things it points at: drop tabs
 * whose target is gone, and refresh titles so a rename shows on the tab.
 *
 * Pruning is skipped while `entries` is empty, on purpose. These lists load
 * lazily (the sidebar only fetches canvases/writing lists when you open that
 * section) and reset to `[]` when their IPC call fails — so "empty" usually
 * means "not loaded yet", not "you deleted everything". Treating it as
 * authoritative would silently wipe restored tabs on a transient error. The cost
 * of the guard is a tab that lingers after you delete the last item of a kind;
 * clicking it just shows that workspace's empty state.
 */
function syncFeatureTabs(
  kind: 'canvas' | 'snippets' | 'writing',
  entries: () => { id: string; name: string }[],
  extraValidIds: string[] = [],
) {
  watch(
    () => entries().map(e => `${e.id}:${e.name}`).join('|'),
    () => {
      const list = entries()
      if (!list.length) return
      readerStore.pruneFeatureTabs(kind, new Set([...extraValidIds, ...list.map(e => e.id)]))
      for (const tab of readerStore.featureTabs(kind)) {
        const entry = list.find(e => e.id === tab.refId)
        if (entry) tab.title = entry.name
      }
    }
  )
}

syncFeatureTabs('writing', () => writingLists.value, [WRITING_ALL])
syncFeatureTabs('canvas', () => canvasStore.canvasList)
syncFeatureTabs('snippets', () => snippetLibraries.value)

function onOpenLibrary() {
  readerStore.showList()
}

// Entering the canvas workspace. With an explicit id (clicking a canvas in the
// sidebar) that canvas gets a tab; without one, re-focus an already-open canvas
// tab, else fall back to the first canvas in the library.
function onOpenCanvas(canvasId?: string) {
  const target = canvasId
    ?? activeCanvasId.value
    ?? openCanvasIds.value[0]
    ?? canvasStore.canvasList[0]?.id
  if (target) activateCanvas(target)
  else readerStore.showList()
  // Land on the drawing panel (and leave the PDF-only tabs behind).
  if (!CANVAS_TABS.includes(sidebarTab.value)) sidebarTab.value = 'draw'
}

function closeCanvas() {
  const id = activeCanvasId.value
  if (id) readerStore.closeTab(featureTabKey('canvas', id))
  // 'draw' only exists in canvas mode — fall back to a paper tab.
  if (!PAPER_TABS.includes(sidebarTab.value)) sidebarTab.value = 'metadata'
}

function activateSnippetLibrary(libraryId: string) {
  const name = snippetLibraries.value.find(lib => lib.id === libraryId)?.name ?? t('snippets.title')
  readerStore.openFeatureTab('snippets', libraryId, name)
}

function onOpenSnippetWorkspace() {
  const target = activeSnippetLibraryId.value
    ?? readerStore.featureTabs('snippets')[0]?.refId
    ?? snippetLibraries.value[0]?.id
  if (target) activateSnippetLibrary(target)
  else readerStore.showList()
}

function onSwitchWorkspace(mode: WorkspaceMode) {
  if (mode === 'library') {
    onOpenLibrary()
  } else if (mode === 'canvas') {
    onOpenCanvas()
  } else if (mode === 'snippets') {
    onOpenSnippetWorkspace()
  } else {
    onOpenWriting()
  }
}

function onOpenSnippetLibrary(libraryId: string) {
  activateSnippetLibrary(libraryId)
}

function onSnippetOpenPaper(slug: string, page: number, title: string) {
  // openPaper makes the paper the active tab, which is all it takes to leave
  // the snippet workspace now that the centre pane follows the active tab.
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
  // Stay on whatever tab is already open. Clicking through the graph with 笔记
  // showing should show each paper's notes — bouncing to the drawing properties
  // every time made the sidebar useless for reading as you navigate. Only a tab
  // that can't render the current context falls back.
  // (`select-paper` also fires when a node is OPENED as a reader tab, by which
  // point this is no longer canvas mode — hence the two tab sets.)
  const allowed = showCanvas.value ? CANVAS_TABS : PAPER_TABS
  if (!allowed.includes(sidebarTab.value)) {
    sidebarTab.value = showCanvas.value ? 'draw' : 'metadata'
  }
}

// Selecting a canvas element reveals the sidebar. A paper node keeps the current
// tab (see above); text/shape/line and multi-selections only have drawing
// properties, so those do jump to 绘图.
watch(
  () => (canvasStore.selectedNodeIds.length ? (canvasStore.selectedNode?.type ?? 'multi') : null),
  (kind) => {
    if (!showCanvas.value || !kind) return
    rightSidebarVisible.value = true
    if (kind !== 'paper' || !CANVAS_TABS.includes(sidebarTab.value)) {
      sidebarTab.value = 'draw'
    }
  }
)

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

// Auto-save tabs whenever they change (length, order, active, or grouping)
watch(
  [() => [...readerStore.tabs], () => readerStore.activeSlug, () => [...readerStore.tabGroups]],
  () => {
    if (libraryStore.currentPath) {
      readerStore.saveTabs(libraryStore.currentPath)
    }
  },
  { deep: true }
)

// Sidebar navigation should always bring the main area back to the library list
// tab. Open tabs stay open; they just stop being the active tab. (Leaving the
// canvas/snippet/writing workspaces falls out of the same call, since the centre
// pane follows the active tab.)
watch(
  () => selectionStore.navSelectionSeq,
  () => { readerStore.showList() }
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
    <!-- The tab bar reads the whole tab list off the reader store now, so it
         needs no per-workspace props: every tab kind lives in `reader.tabs`. -->
    <TabBar
      :right-sidebar-open="rightSidebarVisible"
      @toggle-right-sidebar="rightSidebarVisible = !rightSidebarVisible"
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
          <!-- One panel per open canvas tab, kept mounted and toggled with
               v-show — same shape as the per-slug PdfViewer instances above, so
               switching canvas tabs doesn't reload or lose in-canvas state. -->
          <CanvasPanel
            v-for="id in liveCanvasIds"
            v-show="id === activeCanvasId"
            :key="`canvas:${id}`"
            :canvas-id="id"
            class="center-fill"
            @select-paper="onCanvasSelectPaper"
            @close="closeCanvas()"
          />
          <SnippetLibraryView
            v-if="showSnippetLibrary && activeSnippetLibraryId"
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
          <!-- The canvas panels above are a separate v-for, so the fallback has
               to exclude canvas mode explicitly rather than ride the chain. -->
          <div v-else-if="!showCanvas" class="center-fill">
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
