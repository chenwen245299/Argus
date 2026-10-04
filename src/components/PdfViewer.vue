<script lang="ts">
// Module scope: evaluated once and shared by every PdfViewer in this window (one per open tab).
import {
  planPageRender, planHiddenBudget, hiddenKeepSet, ScrollTracker,
  type PageGeometry, type HiddenLevel, type HiddenViewerState,
} from '../utils/pageRenderPolicy'

// Bitmap memory the on-screen viewer may hold (pixels, 4 bytes each); only ever trims pages
// outside its render zone. A Letter page at 189 % on a 2x display is about 7 MP.
const ACTIVE_BUDGET_PIXELS = 100_000_000
// What all background tabs together may hold. A background tab keeps its canvases - switching
// back must not re-render - so the pile of tabs needs a ceiling of its own.
const HIDDEN_BUDGET_PIXELS = 64_000_000

// Whether the backend has the binary `render_page_image` command. null = not asked yet. Learned
// once per run and shared, so only the first Type 3 page of a stale build pays for the probe.
let rasterBinaryAvailable: boolean | null = null
// The first request is the probe; renders that start meanwhile wait for its answer instead of
// each asking a backend that may not have the command.
let rasterProbe: Promise<unknown> | null = null

interface HiddenHandle {
  hiddenSince: number
  level: () => HiddenLevel
  pixelsNow: () => number
  pixelsAtLevel: () => [number, number, number]
  trimTo: (level: HiddenLevel) => void
}
const hiddenViewers = new Map<symbol, HiddenHandle>()

/** Shrink the longest-hidden tabs until the background tabs fit their shared budget. */
function enforceHiddenBudget() {
  const keys: symbol[] = []
  const states: HiddenViewerState[] = []
  for (const [key, h] of hiddenViewers) {
    states.push({
      id: String(keys.length), hiddenSince: h.hiddenSince, level: h.level(),
      pixelsAtLevel: h.pixelsAtLevel(), pixelsNow: h.pixelsNow(),
    })
    keys.push(key)
  }
  for (const [id, level] of planHiddenBudget(states, HIDDEN_BUDGET_PIXELS)) {
    const h = hiddenViewers.get(keys[Number(id)])
    if (h && level > h.level()) h.trimTo(level)
  }
}
</script>

<script setup lang="ts">
import { ref, shallowRef, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { runTranslation, triggerAskAi } from '../stores/translationHistory'
import { openAddSnippetModal } from '../stores/snippetLibrary'
import { useSpeechStore } from '../stores/speech'
import { spokenSentences, locateSentences, sentenceAt, lineBands, type SpokenSentence, type TextRange } from '../utils/speechFollow'
import {
  spansFromTextLayer, selectedTextBySpan, planFurnitureDropReport, keptSelectionTextAcrossPages,
  estimateBodyFontSize, furniturePageFromTextContent, preferContentGeometry,
  type FurniturePage, type FurnitureDropReport, type SpanReading, type TextContentLike,
} from '../utils/pageFurniture'
import * as pdfjsLib from 'pdfjs-dist'
import type { PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist'
import { EventBus, PDFLinkService } from 'pdfjs-dist/web/pdf_viewer.mjs'
import { useReaderStore } from '../stores/reader'
import { useLibraryStore } from '../stores/library'
import { titleInitialCaps } from '../utils/text'
import { computeSections } from '../utils/sections'
import { renderMarkdown } from '../utils/renderMarkdown'
import { groupHighlights, groupAppearance, indexGroups } from '../utils/highlightGroups'
import { notePopupStyle, clampNotePopupPos, startNotePopupResize, forgetNotePopupSize } from '../utils/notePopup'
import { popupShift } from '../utils/popupFit'
import { fluentIconFor, fluentReady } from '../utils/fluentEmoji'
import type { Highlight, Rect, PaperSections } from '../types'
// The legacy build includes its own Promise.withResolvers polyfill, so the worker
// runs correctly on Ventura (WebKit < 17.4) without a custom wrapper.
import PDFWorkerLegacyUrl from 'pdfjs-dist/legacy/build/pdf.worker.min.mjs?url'

pdfjsLib.GlobalWorkerOptions.workerSrc = PDFWorkerLegacyUrl

// One instance per open tab (MainView renders a v-for of viewers and shows only
// the active one). `slug` is this tab's identity and never changes for an
// instance — always operate on props.slug, not reader.activeSlug (the *visible*
// tab). The instance is destroyed when its tab closes, releasing the PDF.
const props = defineProps<{ slug: string }>()

// ── Store & i18n ──────────────────────────────────────────────────────────────
const reader = useReaderStore()
const library = useLibraryStore()
const speech = useSpeechStore()
const { t } = useI18n()

// True only while THIS tab is the one on screen — used to ignore global
// commands/shortcuts that should hit the active viewer alone.
const isActiveTab = computed(() => reader.activeSlug === props.slug)

// ── Related papers (manual links) ───────────────────────────────────────────
const relatedCount = computed(() =>
  library.papers.find(p => p.slug === props.slug)?.related_ids?.length ?? 0)
const citationEmoji = '🕸️'
const relatedEmoji = '🔗'
const citationEmojiIcon = computed(() => fluentReady.value ? fluentIconFor(citationEmoji) : null)
const relatedEmojiIcon = computed(() => fluentReady.value ? fluentIconFor(relatedEmoji) : null)
function openRelatedFromToolbar(e: MouseEvent) {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  library.openRelatedPopover(props.slug, { x: r.right, y: r.bottom + 4 })
}
function openCitationGraph(e: MouseEvent) {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  library.openCitationGraph(props.slug, { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 })
}

// ── State ──────────────────────────────────────────────────────────────────────
const containerRef = ref<HTMLDivElement | null>(null)
const pageRefs = ref<(HTMLDivElement | null)[]>([])

const pdfDoc = shallowRef<PDFDocumentProxy | null>(null)
const pageCount = ref(0)
const pageSizes = ref<{ width: number; height: number }[]>([]) // at scale=1
const renderedPages = ref<Set<number>>(new Set())
const renderingPages = new Set<number>() // guard against concurrent renders
// Bumped on every scale change so in-flight renders from the old scale can
// detect they're stale and discard their work instead of leaving a page
// rendered at the wrong size (the zoom "ghost page" glitch).
let renderGeneration = 0

// ── Text-layer selection anchoring ──────────────────────────────────────────
// PDF.js's bare TextLayer (used below) renders only the absolutely-positioned
// text spans; it does NOT add the `endOfContent` element that the official
// viewer's TextLayerBuilder relies on to keep drag-selection well-behaved.
// Without it, dragging the mouse into whitespace makes the browser's native
// selection latch onto content in DOM order (e.g. the page title/header),
// producing a wildly over-extended selection. We replicate that mechanism here:
// each text layer gets a hidden `endOfContent` div that we reposition into the
// DOM at the live selection anchor, so dragging into blank space stays anchored
// to the nearest text in reading order.
const selTextLayers = new Map<HTMLElement, HTMLElement>() // textLayer div → endOfContent div
let selListenersAbort: AbortController | null = null
let selPointerDown = false
let selPrevRange: Range | null = null

function resetTextLayerSelecting(end: HTMLElement, textLayer: HTMLElement) {
  textLayer.append(end)
  end.style.width = ''
  end.style.height = ''
  textLayer.classList.remove('selecting')
}

function pruneDetachedTextLayers() {
  // Drop entries for text layers removed by re-render (zoom/scale changes).
  for (const layer of selTextLayers.keys()) {
    if (!layer.isConnected) selTextLayers.delete(layer)
  }
}

function onSelectionChange() {
  const selection = document.getSelection()
  if (!selection || selection.rangeCount === 0) {
    selTextLayers.forEach(resetTextLayerSelecting)
    return
  }
  pruneDetachedTextLayers()

  // Mark which text layers the selection currently intersects.
  const active = new Set<HTMLElement>()
  for (let i = 0; i < selection.rangeCount; i++) {
    const range = selection.getRangeAt(i)
    for (const layer of selTextLayers.keys()) {
      if (!active.has(layer) && range.intersectsNode(layer)) active.add(layer)
    }
  }
  for (const [layer, end] of selTextLayers) {
    if (active.has(layer)) layer.classList.add('selecting')
    else resetTextLayerSelecting(end, layer)
  }

  // Move the endOfContent div next to the live anchor so the browser extends
  // the selection toward it (in reading order) instead of into arbitrary DOM.
  const range = selection.getRangeAt(0)
  const modifyStart = !!selPrevRange &&
    (range.compareBoundaryPoints(Range.END_TO_END, selPrevRange) === 0 ||
     range.compareBoundaryPoints(Range.START_TO_END, selPrevRange) === 0)
  let anchor: Node = modifyStart ? range.startContainer : range.endContainer
  if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode as Node
  if (!modifyStart && range.endOffset === 0) {
    do {
      while (!(anchor as ChildNode).previousSibling) anchor = anchor.parentNode as Node
      anchor = (anchor as ChildNode).previousSibling as Node
    } while (!anchor.childNodes.length)
  }
  const parentTextLayer = (anchor as HTMLElement).parentElement?.closest('.textLayer') as HTMLElement | null
  const end = parentTextLayer ? selTextLayers.get(parentTextLayer) : null
  if (end && parentTextLayer) {
    end.style.width = parentTextLayer.style.width
    end.style.height = parentTextLayer.style.height
    end.style.userSelect = 'text'
    ;(anchor as HTMLElement).parentElement?.insertBefore(
      end, modifyStart ? (anchor as ChildNode) : (anchor as ChildNode).nextSibling)
  }
  selPrevRange = range.cloneRange()
}

// Register a text layer for selection anchoring: append its endOfContent div
// and start `selecting` mode on mousedown so the endOfContent expands to cover
// the layer while dragging.
function setupTextLayerSelection(textLayerDiv: HTMLElement) {
  const end = document.createElement('div')
  end.className = 'endOfContent'
  textLayerDiv.append(end)
  selTextLayers.set(textLayerDiv, end)
  textLayerDiv.addEventListener('mousedown', (e) => {
    // Only begin a selection when the drag starts on an actual glyph span.
    // Starting on blank space (the layer container itself or the endOfContent
    // catch) must be a no-op — otherwise the browser anchors to the nearest
    // text and a tiny drag balloons into a whole-page selection.
    const target = e.target as HTMLElement
    if (target === textLayerDiv || target.classList.contains('endOfContent')) {
      // Blank space: don't begin a new selection (preventDefault stops the
      // browser anchoring to nearby text), but still collapse any existing
      // selection — otherwise preventDefault leaves it (and its popup) up.
      e.preventDefault()
      const sel = window.getSelection()
      if (sel && !sel.isCollapsed) sel.removeAllRanges()
      return
    }
    textLayerDiv.classList.add('selecting')
  })
}

function installSelectionListeners() {
  if (selListenersAbort) return
  selListenersAbort = new AbortController()
  const signal = selListenersAbort.signal
  document.addEventListener('pointerdown', () => { selPointerDown = true }, { signal })
  document.addEventListener('pointerup', () => {
    selPointerDown = false
    selTextLayers.forEach(resetTextLayerSelecting)
  }, { signal })
  window.addEventListener('blur', () => {
    selPointerDown = false
    selTextLayers.forEach(resetTextLayerSelecting)
  }, { signal })
  document.addEventListener('keyup', () => {
    if (!selPointerDown) selTextLayers.forEach(resetTextLayerSelecting)
  }, { signal })
  document.addEventListener('selectionchange', onSelectionChange, { signal })
}

function teardownSelectionListeners() {
  selListenersAbort?.abort()
  selListenersAbort = null
  selTextLayers.clear()
  selPrevRange = null
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const pageRenderTasks = new Map<number, any>() // pdfjs RenderTask per page
const inflightRenders = new Map<number, Promise<void>>()
// The scale each page's DOM was actually built at. A zoom leaves the old content
// on screen (CSS-scaled) rather than blanking it, so "rendered" is no longer the
// same question as "rendered at the current scale" — this is what tells them
// apart and makes a stale page re-render.
const pageRenderScales = new Map<number, number>()
// The devicePixelRatio each page's bitmap was rendered for. Part of the cache key with
// the scale: dragging the window between a Retina panel and an external monitor
// changes the ratio without any zoom, and a page rendered for the old one is shown
// stretched (soft) or needlessly downsampled until it is rendered again.
const pageRenderDprs = new Map<number, number>()

// ── Render scheduling state ───────────────────────────────────────────────────
// What to render and when is decided by utils/pageRenderPolicy from the scroll position;
// the functions that carry it out are under "Render scheduling" further down.
interface RenderCtl {
  /** Set by abortRender: this render must stop and leave nothing behind. */
  cancelled: boolean
  /** Remove everything the render built (eviction / hiding) instead of keeping a committed canvas. */
  discard: boolean
}
const renderCtls = new Map<number, RenderCtl>()
// How a render ended, so the queue can tell a page that keeps failing from one that was cancelled.
const renderOutcome = new Map<number, 'ok' | 'failed' | 'aborted'>()
const failedRenders = new Map<number, number>()
// Renders the queue has started and that have not settled yet (the policy's "in flight").
const startedRenders = new Set<number>()
// Pages still waiting for their turn, best first. Rebuilt from the policy's plan on every
// reconcile, so a stale entry can never outlive the scroll position it was planned for.
const wantQueue: number[] = []
// Pages somebody explicitly asked for (jump target, search hit). Pinned pages are rendered first
// and neither cancelled nor evicted until the pin runs out - a jump renders its target BEFORE it
// scrolls there, so the target is by definition nowhere near the viewport yet.
const pinnedUntil = new Map<number, number>()
// Callers of requestRenderPage waiting for a page; a page somebody waits for is pinned for as long as they wait.
const waiters = new Map<number, { promise: Promise<void>; resolve: () => void; giveUp: ReturnType<typeof setTimeout> }>()
// Bitmap pixels each rendered page holds (feeds the memory budget).
const pagePixelsHeld = new Map<number, number>()
// Pages whose pdf.js operator list is still cached (see renderPage: kept while the page is near).
const opListPages = new Set<number>()
// The pdf.js text content of every rendered page, with the page box it was laid out on. The text layer
// is built from it (so a re-render does not extract the page's text again) and the page-furniture
// classifier takes its GEOMETRY from it rather than from the painted spans: a browser measures span
// widths with its own font metrics (WebKit's run up to 5 % wider than Chrome's), which is what the
// classifier must not depend on. See planSelectionFurniture.
interface PageTextSource { content: TextContentLike; view: number[]; rotate: number }
const pageTextSources = new Map<number, PageTextSource>()
// Object URLs behind the <img> of rasterised pages; revoked when the image leaves the DOM.
const pageObjectUrls = new Map<number, string>()
const scrollTracker = new ScrollTracker()
const viewerKey = Symbol('pdf-viewer')
// scrollTop as last seen by a scroll event: the one value that survives display:none.
let trackedScrollTop = 0
// Height of the viewport when the viewer was last on screen (a hidden container reads 0).
let lastViewportHeight = 0
let reconcileRaf = 0
let scrollIdleTimer: ReturnType<typeof setTimeout> | null = null
let containerResizeObserver: ResizeObserver | null = null
// How far a hidden viewer has been trimmed (0 = not at all).
let hiddenLevel: HiddenLevel = 0
let renderingTornDown = false

/**
 * Ask for a page explicitly and get a promise that settles once it has been rendered (or the
 * attempt is over). Everything automatic goes through the policy; this is for the callers that
 * wait on a page - jumps and search - so the page is pinned and jumps the queue.
 */
function requestRenderPage(idx: number, followRunning = true): Promise<void> {
  // Nothing is rendered for a viewer that is not on screen (see reconcile) or one that is gone;
  // the caller would only be left waiting for a page the queue will not start.
  if (renderingTornDown || !isViewerShown()) return Promise.resolve()
  pinPage(idx)
  // A render already running for this page owns it. Wait on the running render: recording a
  // turned-away call's instantly-settled promise over it hid the real render from the scale
  // watcher, which then stopped waiting for it. That promise also settles when the render is
  // ABORTED (a zoom superseded it, the scheduler cancelled it), with the page still blank, so
  // the caller must not resume on it alone: when the page is not fresh afterwards, ask once more,
  // this time through the waiter below, which only settles once the page really is rendered
  // (or gives up). `followRunning` is what keeps that to a single extra round.
  const running = inflightRenders.get(idx)
  if (followRunning && running && renderingPages.has(idx)) {
    return running.then(() => (isPageFresh(idx) ? undefined : requestRenderPage(idx, false)))
  }
  if (isPageFresh(idx)) return Promise.resolve()
  let w = waiters.get(idx)
  if (!w) {
    let resolve!: () => void
    const promise = new Promise<void>(r => { resolve = r })
    // The caller is about to scroll to this page. If the page cannot be had (the viewer is hidden
    // meanwhile, a render never ends) it must not be left hanging: it would resume - and move the
    // viewport - at some later moment the reader no longer expects.
    const giveUp = setTimeout(() => settleWaiter(idx), WAIT_GIVE_UP_MS)
    w = { promise, resolve, giveUp }
    waiters.set(idx, w)
  }
  const at = wantQueue.indexOf(idx)
  if (at >= 0) wantQueue.splice(at, 1)
  wantQueue.unshift(idx)
  pumpRenderQueue()
  scheduleReconcile()
  return w.promise
}

const scale = ref(1.25)
// True when this PDF uses Type 3 fonts, which pdf.js renders as blank text (e.g.
// figure labels vanish). Such pages are rasterised via the backend PDFium engine
// instead. Set once at load; most modern PDFs stay on the crisp vector path.
const rasterFallback = ref(false)

/** Cheap scan for the Type 3 font subtype marker in the raw PDF bytes. */
function pdfUsesType3(bytes: Uint8Array): boolean {
  const needle = [0x2f, 0x54, 0x79, 0x70, 0x65, 0x33] // "/Type3"
  const n = needle.length
  outer: for (let i = 0, end = bytes.length - n; i <= end; i++) {
    for (let j = 0; j < n; j++) if (bytes[i + j] !== needle[j]) continue outer
    return true
  }
  return false
}
// First-open fit-to-width may be requested while the tab is backgrounded (0
// width); this defers it until the tab becomes visible.
const needsInitialFit = ref(false)
const displayPage = ref(1) // shown in toolbar (1-based)
const pageInputValue = ref('1')
const displayOpenTitle = computed(() =>
  titleInitialCaps(reader.tabs.find(t => t.slug === props.slug)?.title ?? ''))
const PDF_PAGE_MARGIN = 3
const SCROLL_THUMB_INSET = 4
const SCROLL_THUMB_MIN_SIZE = 32
const SCROLL_THUMB_HIDE_DELAY = 650

const scrollThumbs = ref({
  vertical: { visible: false, size: 0, offset: 0 },
  horizontal: { visible: false, size: 0, offset: 0 },
})
const scrollThumbsActive = ref(false)
let scrollThumbHideTimer: ReturnType<typeof setTimeout> | null = null

function zoomStorageKey(slug: string) {
  return `argus:pdf-zoom:${slug}`
}

function loadSavedZoom(slug: string): number | null {
  try {
    const saved = Number(localStorage.getItem(zoomStorageKey(slug)))
    if (Number.isFinite(saved) && saved > 0) return Math.max(0.5, Math.min(4, saved))
  } catch {
    // Ignore storage errors.
  }
  return null // no saved zoom → caller should fitWidth
}

function saveZoom() {
  const slug = props.slug
  if (!slug) return
  try {
    localStorage.setItem(zoomStorageKey(slug), String(scale.value))
  } catch {
    // Best effort only.
  }
}

const error = ref<string | null>(null)
const loading = ref(true)

// Highlight interaction
// rects/text stored at popup-open time so mousedown on color dot can't clear the selection
// A selection can span multiple pages, so rects are grouped per page (each page
// becomes its own highlight). `pages` is ordered by page index.
// `text` / `pages` are what a highlight (or translate / ask-AI / read-aloud) will use. For a
// selection across a page break the page furniture (page numbers, running heads, footnotes,
// margin text) and the figures and tables it runs through are left out of them (`furniture`
// says what was). `full` is the whole selection across pages, its text rebuilt from the text
// layers (see planSelectionFurniture): ⌘C checks against it that the selection is still this one.
interface SelectionVariant { text: string; pages: { pageIndex: number; rects: Rect[] }[] }
const selectionPopup = ref<{
  x: number
  y: number
  text: string
  pages: { pageIndex: number; rects: Rect[] }[]
  full?: SelectionVariant
  furniture?: FurnitureDropReport
} | null>(null)
const activeColor = ref('#FFEB3B') // default yellow

const HIGHLIGHT_STYLE_KEY = 'argus:highlight-style'
const highlightStyle = ref<'highlight' | 'underline'>(
  (localStorage.getItem(HIGHLIGHT_STYLE_KEY) as 'highlight' | 'underline' | null) ?? 'highlight'
)
function toggleHighlightStyle() {
  highlightStyle.value = highlightStyle.value === 'highlight' ? 'underline' : 'highlight'
  localStorage.setItem(HIGHLIGHT_STYLE_KEY, highlightStyle.value)
}
// `ids` = every record of the highlight when the popup opened; see `resolveHighlightGroup`.
const hlNotePopup = ref<{ x: number; y: number; hlId: string; ids: string[] } | null>(null)   // left-click: note view/edit
const hlNoteText = ref('')
const hlNoteEditing = ref(false)   // false = view mode, true = edit mode
const noteTextareaRef = ref<HTMLTextAreaElement | null>(null)
const hlColorPopup = ref<{ x: number; y: number; hlId: string; ids: string[] } | null>(null)  // right-click: color + delete

// Notes are authored as markdown + $TeX$ and rendered on the view side, so a
// formula reads as a formula instead of raw source (same deal as the notes tab).
const hlNoteHtml = computed(() => renderMarkdown(hlNoteText.value))

// Position is already clamped at open time; only the size stays reactive here.
const hlNotePopupStyle = computed(() => {
  const p = hlNotePopup.value
  return p ? notePopupStyle(p.x, p.y, p.hlId) : {}
})

function openNotePopup(x: number, y: number, hlId: string, ids: string[]) {
  hlNotePopup.value = { ...clampNotePopupPos(x, y, hlId), hlId, ids }
}

// Sizes are stored per-highlight, so the drag always names the highlight the
// popup is currently showing rather than whatever it was bound to at mount.
function onNoteResizeStart(e: PointerEvent) {
  const p = hlNotePopup.value
  if (!p) return
  e.stopPropagation()
  startNotePopupResize(e, p.hlId, { x: p.x, y: p.y })
}

const COLORS = computed(() => [
  { label: t('pdf.yellow'), value: '#FFEB3B' },
  { label: t('pdf.green'),  value: '#A5D6A7' },
  { label: t('pdf.blue'),   value: '#90CAF9' },
  { label: t('pdf.pink'),   value: '#F48FB1' },
  { label: t('pdf.orange'), value: '#FFCC80' },
  { label: t('pdf.purple'), value: '#CE93D8' },
])

// Debounce timer for reading state
let progressDebounce: ReturnType<typeof setTimeout> | null = null

// ── In-document jump history ──────────────────────────────────────────────────
// Following a cross-reference ("see Wu et al., 2021") throws away where you were
// reading, which is the whole reason you followed it. So every link jump records
// the spot it left, and ⌘[ / ⌘] (plus the mouse's side buttons) walk that history
// exactly like a browser's back/forward: back pushes the current spot onto the
// forward stack, and a fresh jump discards the forward stack.

interface JumpPos {
  scrollTop: number
  scrollLeft: number
  /** Zoom at capture time — the position is rescaled if the user zoomed since. */
  scale: number
}

const JUMP_HISTORY_LIMIT = 50
const jumpBack = ref<JumpPos[]>([])
const jumpForward = ref<JumpPos[]>([])

function currentJumpPos(): JumpPos | null {
  const el = containerRef.value
  if (!el) return null
  return { scrollTop: el.scrollTop, scrollLeft: el.scrollLeft, scale: scale.value }
}

function restoreJumpPos(pos: JumpPos) {
  const el = containerRef.value
  if (!el) return
  // Page wrappers keep their box even when not rendered, so scrollTop stays
  // meaningful; only zoom changes it, and that's a plain ratio.
  const ratio = scale.value / (pos.scale || scale.value)
  el.scrollTop = pos.scrollTop * ratio
  el.scrollLeft = pos.scrollLeft * ratio
}

/** Records the spot a link is leaving. Called right as the jump commits. */
function pushJumpOrigin(pos: JumpPos) {
  jumpBack.value.push(pos)
  if (jumpBack.value.length > JUMP_HISTORY_LIMIT) jumpBack.value.shift()
  jumpForward.value = []
  showJumpHint()
}

function jumpHistoryBack() {
  const target = jumpBack.value.pop()
  if (!target) return
  const here = currentJumpPos()
  if (here) jumpForward.value.push(here)
  restoreJumpPos(target)
}

function jumpHistoryForward() {
  const target = jumpForward.value.pop()
  if (!target) return
  const here = currentJumpPos()
  if (here) jumpBack.value.push(here)
  restoreJumpPos(target)
}

// ── Jump hint toast ───────────────────────────────────────────────────────────
const JUMP_HINT_DISMISSED_KEY = 'argus:pdf-jump-hint-dismissed'
const jumpHintVisible = ref(false)
let jumpHintTimer: ReturnType<typeof setTimeout> | null = null

const isMacPlatform = navigator.userAgent.toLowerCase().includes('mac')
const jumpModLabel = computed(() => (isMacPlatform ? '⌘' : 'Ctrl+'))

function jumpHintSuppressed(): boolean {
  try {
    return localStorage.getItem(JUMP_HINT_DISMISSED_KEY) === '1'
  } catch {
    return false
  }
}

function clearJumpHintTimer() {
  if (jumpHintTimer) { clearTimeout(jumpHintTimer); jumpHintTimer = null }
}

function showJumpHint() {
  if (jumpHintSuppressed() || !isActiveTab.value) return
  jumpHintVisible.value = true
  clearJumpHintTimer()
  jumpHintTimer = setTimeout(() => { jumpHintVisible.value = false }, 5000)
}

function hideJumpHint() {
  clearJumpHintTimer()
  jumpHintVisible.value = false
}

function dismissJumpHintForever() {
  try {
    localStorage.setItem(JUMP_HINT_DISMISSED_KEY, '1')
  } catch {
    // a non-persistent dismissal is still better than ignoring the click
  }
  hideJumpHint()
}

// PDF.js link/annotation handling.
//
// `SimpleLinkService` is a navigation no-op — its `setDocument`/`goToDestination`
// do nothing — so internal jump links (table-of-contents entries, cross-references,
// "see figure N") never moved the viewport. This subclass of `PDFLinkService`
// resolves each destination to a page + Y offset and drives our own page scroller,
// so those links actually jump to the target.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
class ArgusLinkService extends PDFLinkService {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async goToDestination(dest: string | any[]) {
    const doc = pdfDoc.value
    if (!doc) return
    // Captured before the awaits below — none of them move the viewport, so
    // this is still the spot the reader is leaving.
    const origin = currentJumpPos()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let explicitDest: any[] | null
    try {
      explicitDest = typeof dest === 'string' ? await doc.getDestination(dest) : await dest
    } catch {
      return
    }
    if (!Array.isArray(explicitDest)) return

    const destRef = explicitDest[0]
    let pageIndex: number | null = null
    if (destRef && typeof destRef === 'object') {
      try {
        pageIndex = await doc.getPageIndex(destRef)
      } catch {
        return
      }
    } else if (Number.isInteger(destRef)) {
      pageIndex = destRef as number
    }
    if (pageIndex === null || pageIndex < 0 || pageIndex >= doc.numPages) return

    // Resolve the target Y (scale-1 CSS pixels from the page top) from the
    // destination's fit spec; fall back to the page top when it has no coordinate.
    let offsetY = 0
    try {
      const page = await doc.getPage(pageIndex + 1)
      offsetY = destOffsetY(page.getViewport({ scale: 1 }), explicitDest)
      page.cleanup()
    } catch {
      // fall back to page top
    }

    await ensurePageRendered(pageIndex)
    if (origin) pushJumpOrigin(origin)
    scrollToPageIndex(pageIndex, offsetY)
  }

  goToPage(val: number | string) {
    const doc = pdfDoc.value
    if (!doc) return
    const pageNumber = typeof val === 'number' ? val : parseInt(val, 10)
    if (!Number.isInteger(pageNumber) || pageNumber < 1 || pageNumber > doc.numPages) return
    const origin = currentJumpPos()
    void ensurePageRendered(pageNumber - 1).then(() => {
      if (origin) pushJumpOrigin(origin)
      scrollToPageIndex(pageNumber - 1, 0)
    })
  }

  executeNamedAction(action: string) {
    const doc = pdfDoc.value
    if (!doc) return
    switch (action) {
      case 'NextPage': this.goToPage(displayPage.value + 1); break
      case 'PrevPage': this.goToPage(displayPage.value - 1); break
      case 'FirstPage': this.goToPage(1); break
      case 'LastPage': this.goToPage(doc.numPages); break
      default: break
    }
  }
}

const eventBus = new EventBus()
const linkService = new ArgusLinkService({ eventBus, externalLinkTarget: 2 })

// ── Computed ──────────────────────────────────────────────────────────────────
const sortedHighlights = computed(() => {
  return [...reader.highlightsFor(props.slug)].sort((a, b) => {
    if (a.page !== b.page) return a.page - b.page
    const aY = a.rects[0]?.y ?? 0
    const bY = b.rects[0]?.y ?? 0
    return aY - bY
  })
})

function pageHighlights(pageIndex: number): Highlight[] {
  // pageIndex is 0-based; Highlight.page is 1-based
  return reader.highlightsFor(props.slug).filter(h => h.page === pageIndex + 1)
}

// A selection across a page break is stored as one record per page (each draws only
// its own page's rects), but it is ONE highlight to the user: clicking either half
// must open the same note, and colour / delete / copy must act on the whole thing.
// `highlightGroupIndex` maps any record id to its group; the popups are keyed by the
// group's canonical id so both halves share one popup (and one remembered size).
const highlightGroupIndex = computed(() =>
  indexGroups(groupHighlights(reader.highlightsFor(props.slug))))

// The popups are keyed by the canonical id and stay open across a reload (window
// focus and file-watch events replace the whole array). If that record is gone by
// the time an action runs — an older build deleted the lowest-page half on another
// machine — the key resolves to nothing although the other half is still there, and
// the action would silently hit no record. So the popup also remembers every id the
// highlight had when it opened, and the group is found through whichever is left.
function resolveHighlightGroup(id: string) {
  const known = hlColorPopup.value?.hlId === id ? hlColorPopup.value.ids
    : hlNotePopup.value?.hlId === id ? hlNotePopup.value.ids : []
  for (const k of [id, ...known]) {
    const g = highlightGroupIndex.value.get(k)
    if (g) return g
  }
  return undefined
}

/** Ids of every record of the highlight `id` belongs to (just `[id]` if it stands alone). */
function highlightIdsOf(id: string): string[] {
  return resolveHighlightGroup(id)?.ids ?? [id]
}

/** The highlight the right-click menu is open on (drives its line-break toggle). */
const hlMenuGroup = computed(() =>
  hlColorPopup.value ? resolveHighlightGroup(hlColorPopup.value.hlId) : undefined)

function openHighlightNote(anchor: DOMRect, id: string) {
  const g = highlightGroupIndex.value.get(id)
  hlNoteText.value = g?.note ?? ''
  hlNoteEditing.value = false
  openNotePopup(anchor.left, anchor.bottom + 4, g?.id ?? id, g?.ids ?? [id])
  hlColorPopup.value = null
}

const hlMenuRef = ref<HTMLElement | null>(null)

async function openHighlightMenu(e: MouseEvent, id: string) {
  const g = highlightGroupIndex.value.get(id)
  hlColorPopup.value = { x: e.clientX, y: e.clientY + 4, hlId: g?.id ?? id, ids: g?.ids ?? [id] }
  hlNotePopup.value = null
  // The menu sits at the click point, and its width depends on the labels (longer in
  // English, and wider still with the line-break toggle), so keep it inside the window
  // once it is laid out rather than letting Translate / Delete run off the edge.
  await nextTick()
  const menu = hlMenuRef.value
  const open = hlColorPopup.value
  if (!menu || !open) return
  const { width, height } = menu.getBoundingClientRect()
  const x = Math.max(8, Math.min(open.x, window.innerWidth - width - 8))
  const y = Math.max(8, Math.min(open.y, window.innerHeight - height - 8))
  if (x !== open.x || y !== open.y) hlColorPopup.value = { ...open, x, y }
}

// The selection toolbar is placed at the mouse position, but its width varies (language, the
// read-aloud button, the page-furniture toggle) so, like the menu above, it is measured once laid
// out and pulled back inside the window. Only on open and when its own size changes - never on
// scroll, where it follows its anchor instead (repositionAnchoredPopups).
const selPopupRef = ref<HTMLElement | null>(null)
let selPopupObserver: ResizeObserver | null = null

function fitSelectionPopup() {
  const el = selPopupRef.value
  const open = selectionPopup.value
  if (!el || !open) return
  const { dx, dy } = popupShift(
    el.getBoundingClientRect(),
    { width: window.innerWidth, height: window.innerHeight },
    // 12 px = the gap between the cursor and the toolbar (see onWindowMouseUp)
    { margin: 8, flipGap: 12 },
  )
  if (dx || dy) selectionPopup.value = { ...open, x: open.x + dx, y: open.y + dy }
}

// The element exists only while a popup is open; a size change (the toggle's label differs between
// its two states) must re-fit it as well.
watch(selPopupRef, (el) => {
  selPopupObserver?.disconnect()
  if (!el || typeof ResizeObserver === 'undefined') return
  selPopupObserver ??= new ResizeObserver(() => fitSelectionPopup())
  selPopupObserver.observe(el)
}, { flush: 'post' })

// ── Lifecycle ──────────────────────────────────────────────────────────────────
// Every open tab has a live viewer instance, but only the VISIBLE one may own the
// global (window/document) listeners — otherwise every open viewer would react to
// the same keypress/selection. So they follow the active state, not mount/unmount.
function addGlobalListeners() {
  installSelectionListeners()
  window.addEventListener('mouseup', onWindowMouseUp)
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('mousedown', onWindowMouseDown)
  document.addEventListener('copy', onCopySelection)
  window.addEventListener('argus-snippet-highlight', onSnippetHighlight)
  window.addEventListener('resize', updateScrollThumbs)
}

function removeGlobalListeners() {
  teardownSelectionListeners()
  window.removeEventListener('mouseup', onWindowMouseUp)
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('mousedown', onWindowMouseDown)
  document.removeEventListener('copy', onCopySelection)
  window.removeEventListener('argus-snippet-highlight', onSnippetHighlight)
  window.removeEventListener('resize', updateScrollThumbs)
}

/** True while a drag that began inside the note popup is in flight. */
let noteDragOrigin = false
/** The current press began inside one of the popups (a colour dot, a button), not on the page. */
let pressInPopup = false

function onWindowMouseDown(e: MouseEvent) {
  noteDragOrigin = !!(e.target as HTMLElement).closest?.('.hl-note-popup')
  pressInPopup = !!(e.target as HTMLElement).closest?.('.hl-note-popup, .hl-color-popup, .sel-popup')
  // A press on the page starts over: the spans the last selection left out are plain text again.
  if (!pressInPopup) clearSkippedSpans()
  // Mouse side buttons: 3 = back, 4 = forward.
  if (e.button !== 3 && e.button !== 4) return
  e.preventDefault()
  if (e.button === 3) jumpHistoryBack()
  else jumpHistoryForward()
}

onMounted(async () => {
  watchDevicePixelRatio()
  await loadPdf()
})

// `resolution` media queries are the only change signal for devicePixelRatio: match the
// current ratio exactly, and re-arm for the new one whenever it stops matching.
let dprQuery: MediaQueryList | null = null
function watchDevicePixelRatio() {
  dprQuery?.removeEventListener('change', onDevicePixelRatioChange)
  dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
  dprQuery.addEventListener('change', onDevicePixelRatioChange)
}
function onDevicePixelRatioChange() {
  watchDevicePixelRatio()
  // The render cache is keyed on the ratio, so the plan sees every page drawn for the old
  // one as stale and re-renders those in the render zone. A backgrounded viewer catches up
  // when it is next shown (onViewerShown plans then).
  failedRenders.clear()
  if (!isActiveTab.value) return
  scheduleReconcile()
}

// Backgrounded viewers are hidden with v-show (display:none), which can drop the
// scroll position — stash it on hide and restore on show.
let _savedScrollTop: number | null = null
// The PDF finished loading while the tab was in the background: a hidden container cannot be
// scrolled, so the reading position saved on disk is applied when the tab is first shown.
let restorePending = false

watch(isActiveTab, (active) => {
  if (active) {
    addGlobalListeners()
    void showViewer()
  } else {
    // Once display:none has landed, scrollTop reads 0 — trust the value tracked from the
    // scroll events in that case.
    const c = containerRef.value
    if (c) {
      _savedScrollTop = c.clientHeight === 0 ? trackedScrollTop : c.scrollTop
      // A scroll the reader made in this very tick has not produced its event yet, and the event
      // that does arrive once the container is hidden is ignored: this is the position.
      trackedScrollTop = _savedScrollTop
    }
    removeGlobalListeners()
    hideJumpHint() // don't let it reappear when this tab comes back
    // Persist the scroll position before this tab goes to the background, and drop the pending
    // save: it would fire with the container hidden (and the page it reports is whatever the
    // tab hidden at, not where the reader is).
    if (progressDebounce) { clearTimeout(progressDebounce); progressDebounce = null }
    flushReadingState()
    onViewerHidden()
  }
}, { immediate: true })

/**
 * The tab came to the front. The parent's v-show is applied in the flush that triggered this, so
 * wait for it; everything after that still runs before the browser paints (microtasks), so the
 * first frame already shows the page the reader left, with the canvases it kept.
 */
async function showViewer() {
  await nextTick()
  const c = containerRef.value
  // Still loading: loadPdf restores and plans for itself when it is done.
  if (!c || !pdfDoc.value || renderingTornDown || !isActiveTab.value) return
  // Complete a first-open fit-to-width that was deferred because the tab was still
  // backgrounded (0 width) when the PDF finished loading. It changes the scale, so the page
  // boxes only have their final size one flush later - and a position worked out before that
  // would land on the wrong page.
  if (needsInitialFit.value) {
    fitWidth()
    await nextTick()
  }
  if (restorePending) {
    restorePending = false
    restorePosition()
  } else if (_savedScrollTop !== null) {
    c.scrollTop = _savedScrollTop
  }
  // Restore the scroll position FIRST, then plan: nothing is rendered or evicted for a
  // position the viewer is only about to leave.
  onViewerShown()
}

// Fires when the tab is closed (removed from the open-tabs list) — the PDF is
// fully released here.
onUnmounted(() => {
  removeGlobalListeners()
  teardownRenderScheduling()
  dprQuery?.removeEventListener('change', onDevicePixelRatioChange)
  if (progressDebounce) clearTimeout(progressDebounce)
  if (scrollThumbHideTimer) clearTimeout(scrollThumbHideTimer)
  if (searchDebounce) clearTimeout(searchDebounce)
  clearJumpHintTimer()
  selPopupObserver?.disconnect()
  pageTextCache.clear()
  pageTextSources.clear()
  pdfDoc.value?.destroy()
  // This viewer is gone for good (tab closed or evicted from cache) — drop its
  // cached highlights/reading-state from the store.
  reader.discardTabState(props.slug)
})

// ── Inline translation ────────────────────────────────────────────────────────
// The streaming + model-resolution + usage tracking lives in the translation
// store so the translation tab (regenerate) can drive it too.
async function translateSelection() {
  if (!selectionPopup.value) return
  const { text } = selectionPopup.value
  selectionPopup.value = null
  await runTranslation(text)
}

function askAiWithSelection() {
  if (!selectionPopup.value) return
  const { text } = selectionPopup.value
  selectionPopup.value = null
  triggerAskAi(text)
}

/** The popup's text is the one being read right now: the button then reads 停止朗读. */
const readingSelection = computed(() =>
  !!selectionPopup.value && speech.isReadingText(selectionPopup.value.text, 'pdf'))

// Read aloud (朗读). `speech.read` must run in the click's own call stack — the audio
// element is unlocked there, before any network wait (see stores/speech.ts) — so nothing
// here awaits ahead of it. Not configured yet: it opens the "set up a voice" prompt instead.
function readAloudSelection(source: 'pdf' | 'ebook') {
  if (!selectionPopup.value) return
  const { text } = selectionPopup.value
  // The page text the selection came from, so the sentence being spoken can be lit on the page.
  const pageText = popupSource ? captureReadingText(popupSource.range, popupSource.skip) : null
  selectionPopup.value = null
  readingTrack = pageText ? { text, source, pageText } : null
  void speech.read(text, { source, paper: props.slug })
  // The selection's own tint would sit under the sentence being lit; the popup is gone, so is it.
  if (pageText) window.getSelection()?.removeAllRanges()
}

// ── Read aloud: the sentence being spoken, lit on the page ───────────────────
// A read started here is followed sentence by sentence (utils/speechFollow.ts): its sentences
// are found in the text layer once, when the read starts, and turned into page rectangles
// (scale-1 coordinates, like a highlight's), so zooming and re-rendering only redraw them.
// Which one is lit follows the player's playhead. Another read, a stop or the end clears it.

const READING_COLOR = '#A78BFA'

/** The selection the popup was opened for: the popup keeps only its text, the read needs where it is. */
let popupSource: { range: Range; skip: ReadonlySet<HTMLElement> | null } | null = null

/** The selected text as the text layer holds it: its text nodes in order, skipped spans left out. */
interface ReadingText {
  text: string
  nodes: { node: Text; start: number; at: number; len: number }[]
  skip: ReadonlySet<HTMLElement> | null
}

/** The read this viewer started, until it is over or replaced. */
let readingTrack: { text: string; source: string; pageText: ReadingText } | null = null
/** Its sentences, each with where it sits on the pages; null while this viewer is not being read. */
let readingSentences: (SpokenSentence & { pages: { pageIndex: number; rects: Rect[] }[] })[] | null = null
let readingSentenceIdx = -1

function captureReadingText(range: Range, skip: ReadonlySet<HTMLElement> | null): ReadingText | null {
  const root = range.commonAncestorContainer
  const rootEl = (root.nodeType === Node.ELEMENT_NODE ? root : root.parentNode) as HTMLElement | null
  if (!rootEl || !rootEl.isConnected) return null
  const walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (!node.nodeValue) return NodeFilter.FILTER_REJECT
      return range.intersectsNode(node) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT
    },
  })
  let text = ''
  const nodes: ReadingText['nodes'] = []
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const parent = node.parentElement
    if (!parent?.closest('.textLayer') || skip?.has(parent)) continue
    const value = node.nodeValue ?? ''
    const start = node === range.startContainer ? range.startOffset : 0
    const end = node === range.endContainer ? range.endOffset : value.length
    if (end <= start) continue
    nodes.push({ node: node as Text, start, at: text.length, len: end - start })
    text += value.slice(start, end)
  }
  return nodes.length ? { text, nodes, skip } : null
}

/** A place in the captured text as a DOM position; an end may sit at the very end of a node. */
function readingPoint(t: ReadingText, at: number, isEnd: boolean): { node: Text; offset: number } | null {
  for (const n of t.nodes) {
    if (isEnd ? at > n.at && at <= n.at + n.len : at >= n.at && at < n.at + n.len) {
      return { node: n.node, offset: n.start + (at - n.at) }
    }
  }
  return null
}

function readingRects(t: ReadingText, r: TextRange): { pageIndex: number; rects: Rect[] }[] {
  const a = readingPoint(t, r.start, false)
  const b = readingPoint(t, r.end, true)
  if (!a || !b || !a.node.isConnected || !b.node.isConnected) return []
  const range = document.createRange()
  try {
    range.setStart(a.node, a.offset)
    range.setEnd(b.node, b.offset)
  } catch {
    return []
  }
  return collectSelectionRectsByPage(range, t.skip ?? undefined)
    .map(({ pageIndex, rects }) => ({ pageIndex, rects: lineBands(rects) }))
}

function pagesOfReadingSentence(i: number): number[] {
  return readingSentences?.[i]?.pages.map(p => p.pageIndex) ?? []
}

function redrawReadingPages(pages: Iterable<number>) {
  for (const idx of new Set(pages)) {
    const overlay = pageRefs.value[idx]?.querySelector('.highlight-overlay') as HTMLDivElement | null
    if (overlay) drawReadingHighlight(overlay, idx)
  }
}

function showReadingSentence(i: number) {
  if (i === readingSentenceIdx) return
  const before = pagesOfReadingSentence(readingSentenceIdx)
  readingSentenceIdx = i
  redrawReadingPages([...before, ...pagesOfReadingSentence(i)])
}

function clearReading() {
  const before = pagesOfReadingSentence(readingSentenceIdx)
  readingSentences = null
  readingSentenceIdx = -1
  readingTrack = null
  redrawReadingPages(before)
}

/** Draws the sentence being spoken on one page (or nothing); renderHighlightsOnPage calls it last. */
function drawReadingHighlight(container: HTMLDivElement, pageIndex: number) {
  container.querySelector(':scope > .reading-hl')?.remove()
  const rects = readingSentences?.[readingSentenceIdx]?.pages.find(p => p.pageIndex === pageIndex)?.rects
  if (!rects?.length) return
  const s = scale.value
  const NS = 'http://www.w3.org/2000/svg'
  const svg = document.createElementNS(NS, 'svg') as SVGSVGElement
  svg.setAttribute('class', 'reading-hl')
  svg.style.cssText = 'position:absolute;top:0;left:0;width:100%;height:100%;overflow:visible;pointer-events:none'
  // One group at one opacity, like a highlight: overlapping boxes do not stack darker.
  const g = document.createElementNS(NS, 'g') as SVGGElement
  g.setAttribute('fill', READING_COLOR)
  for (const rect of rects) {
    const r = document.createElementNS(NS, 'rect') as SVGRectElement
    r.setAttribute('x', String(rect.x * s))
    r.setAttribute('y', String(rect.y * s))
    r.setAttribute('width', String(rect.width * s))
    r.setAttribute('height', String(rect.height * s))
    r.setAttribute('rx', '2')
    g.appendChild(r)
  }
  svg.appendChild(g)
  container.appendChild(svg)
}

// A new read (here or anywhere), a stop, the end: `readChunks` changes with each.
watch(() => speech.readChunks, (chunks) => {
  const track = readingTrack
  if (!track || chunks.length === 0 || !speech.isReadingText(track.text, track.source)) {
    if (readingSentences || readingTrack) clearReading()
    return
  }
  const sentences = spokenSentences(chunks)
  const ranges = locateSentences(track.pageText.text, sentences.map(x => x.text))
  const before = pagesOfReadingSentence(readingSentenceIdx)
  readingSentences = sentences.map((x, i) => ({ ...x, pages: ranges[i] ? readingRects(track.pageText, ranges[i]!) : [] }))
  readingSentenceIdx = sentenceAt(readingSentences, speech.playhead.index, speech.playhead.fraction)
  redrawReadingPages([...before, ...pagesOfReadingSentence(readingSentenceIdx)])
})

watch(() => speech.playhead, (p) => {
  if (readingSentences) showReadingSentence(sentenceAt(readingSentences, p.index, p.fraction))
})

const SNIPPET_HIGHLIGHT_COLOR = '#CE93D8'

function addToSnippetLibrary() {
  const popup = selectionPopup.value
  if (!popup || popup.pages.length === 0) { selectionPopup.value = null; return }
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
  const paper = library.papers.find(p => p.slug === props.slug)
  // A snippet anchors to a single page — use the page the selection starts on.
  const first = popup.pages[0]
  openAddSnippetModal({
    text: popup.text,
    paperId: props.slug,
    paperTitle: paper?.title ?? props.slug,
    page: first.pageIndex + 1,
    color: SNIPPET_HIGHLIGHT_COLOR,
    rects: first.rects,
    pageIndex: first.pageIndex,
  })
}

function onSnippetHighlight(e: Event) {
  const { rects, pageIndex, text, color } = (e as CustomEvent).detail
  if (!rects?.length) return
  const hl: Highlight = {
    id: crypto.randomUUID(),
    page: pageIndex + 1,
    rects,
    text,
    color,
    created_at: new Date().toISOString(),
    style: 'highlight',
  }
  reader.addHighlight(hl)
}

function addHighlightToSnippetLibrary(hlId: string) {
  const hl = resolveHighlightGroup(hlId)
  if (!hl) return
  hlColorPopup.value = null
  const paper = library.papers.find(p => p.slug === props.slug)
  openAddSnippetModal({
    text: hl.displayText,
    paperId: props.slug,
    paperTitle: paper?.title ?? props.slug,
    page: hl.page,
    color: hl.color,
  })
}

function onAnnotationLayerClick(e: MouseEvent) {
  const target = e.target as HTMLElement
  const link = target.closest('a')
  if (!link) return
  // Internal jump links (GoTo destinations, named actions) carry `data-internal-link`
  // on their container section. The annotation layer already wired their <a>.onclick
  // to our link service, so let that handle navigation — don't treat them as URLs.
  if (link.closest('[data-internal-link]')) return
  const href = link.getAttribute('href')
  if (!href) return
  e.preventDefault()
  e.stopPropagation()
  openExternalLink(href)
}

async function openExternalLink(url: string) {
  try {
    await invoke('open_url', { url })
  } catch (e) {
    console.error('Failed to open URL:', e)
  }
}

// Convert a PDF destination array's target position into a scale-1 CSS-pixel
// offset from the top of the page. Supports the common fit types that carry a
// vertical anchor (XYZ/FitH/FitBH/FitR); other types scroll to the page top.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function destOffsetY(viewport: any, explicitDest: any[]): number {
  const fit = explicitDest[1]
  if (!fit || typeof fit !== 'object') return 0
  let top: number | null = null
  switch (fit.name) {
    case 'XYZ': top = explicitDest[3]; break
    case 'FitH':
    case 'FitBH': top = explicitDest[2]; break
    case 'FitR': top = explicitDest[5]; break
    default: return 0
  }
  if (typeof top !== 'number') return 0
  try {
    const [, vy] = viewport.convertToViewportPoint(0, top)
    return Math.max(0, vy)
  } catch {
    return 0
  }
}

/** Do two rects overlap by a meaningful fraction (ignoring hairline edge touches)? */
function rectsOverlap(a: DOMRect, b: DOMRect): boolean {
  const ix = Math.min(a.right, b.right) - Math.max(a.left, b.left)
  const iy = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top)
  if (ix <= 1 || iy <= 1) return false
  const smaller = Math.min(a.width * a.height, b.width * b.height)
  return smaller > 0 && (ix * iy) / smaller > 0.3
}

function linkifyTextLayer(
  textLayer: HTMLDivElement,
  overlay: HTMLDivElement,
  annotationLayer?: HTMLDivElement,
) {
  const spans = Array.from(textLayer.querySelectorAll('span')).filter(
    s => !s.querySelector('span')
  ) as HTMLSpanElement[]
  const overlayRect = overlay.getBoundingClientRect()

  // Rects already owned by real PDF link annotations (hyperref etc.). A wrapped
  // URL keeps ONE annotation carrying the full target across both lines; laying a
  // second, line-truncated linkify box on top of it would steal the click and send
  // the user to just the first line's fragment. Skip anything an annotation covers.
  const annotationRects: DOMRect[] = annotationLayer
    ? Array.from(annotationLayer.querySelectorAll<HTMLElement>('a')).map(a =>
        a.getBoundingClientRect()
      )
    : []
  const coveredByAnnotation = (r: DOMRect) => annotationRects.some(a => rectsOverlap(a, r))

  // Text nodes in reading order so a URL broken across lines can be stitched back
  // together from the following span(s).
  const nodes = spans.map(s =>
    s.firstChild?.nodeType === Node.TEXT_NODE ? (s.firstChild as Text) : null
  )

  const urlRegex = /https?:\/\/[^\s<>"{}|\\^`[\]]+/gi
  for (let i = 0; i < spans.length; i++) {
    const node = nodes[i]
    if (!node) continue
    const text = node.data
    let match: RegExpExecArray | null
    urlRegex.lastIndex = 0
    while ((match = urlRegex.exec(text)) !== null) {
      const start = match.index
      const firstEnd = start + match[0].length
      // Clickable ranges making up this (possibly multi-line) URL.
      const pieces: { node: Text; start: number; end: number }[] = [
        { node, start, end: firstEnd },
      ]
      let full = match[0]

      // Follow the URL onto the next line(s) when it was broken mid-token. A break
      // leaves the fragment flush against the end of its line (nothing after it),
      // so only pursue continuations when the match reaches the end of the span.
      let k = i
      let reachedEnd = firstEnd === text.length
      while (reachedEnd) {
        const nextNode = nodes[k + 1]
        const nextSpan = spans[k + 1]
        if (!nextNode || !nextSpan) break
        // The continuation must sit on a lower line (a wrap), not later on the same
        // line — that filters out prose that merely follows a complete URL.
        const curTop = spans[k].getBoundingClientRect().top
        const nextTop = nextSpan.getBoundingClientRect().top
        if (nextTop <= curTop + 1) break
        const nextText = nextNode.data
        const tok = nextText.match(/^\S+/)?.[0] ?? ''
        if (!tok) break
        if (/^https?:\/\//i.test(tok)) break // a fresh URL (e.g. a reference list), not a continuation
        if (!/\/|\.[A-Za-z0-9]/.test(tok)) break // doesn't look like a domain/path continuation
        full += tok
        pieces.push({ node: nextNode, start: 0, end: tok.length })
        reachedEnd = tok.length === nextText.length // whole next line was URL → it may wrap again
        k++
      }

      // Strip trailing sentence punctuation from the assembled URL (safe now — any
      // mid-URL dots that caused the line break are interior, not trailing).
      const stripped = full.replace(/[.,;:!?)\]}"'`]+$/, '')
      const dropped = full.length - stripped.length
      if (!stripped) continue
      if (dropped > 0) {
        const last = pieces[pieces.length - 1]
        last.end = Math.max(last.start, last.end - dropped)
        if (last.end === last.start) pieces.pop()
      }

      for (const p of pieces) {
        const range = document.createRange()
        range.setStart(p.node, p.start)
        range.setEnd(p.node, p.end)
        for (const rect of range.getClientRects()) {
          if (coveredByAnnotation(rect)) continue
          createTextLinkOverlay(overlay, rect, overlayRect, stripped)
        }
      }
    }
  }
}

function createTextLinkOverlay(
  overlay: HTMLDivElement,
  rect: DOMRect,
  overlayRect: DOMRect,
  url: string,
) {
  const a = document.createElement('a')
  a.href = url
  a.className = 'text-link'
  a.target = '_blank'
  a.rel = 'noopener noreferrer nofollow'
  a.style.position = 'absolute'
  a.style.left = `${rect.left - overlayRect.left}px`
  a.style.top = `${rect.top - overlayRect.top}px`
  a.style.width = `${rect.width}px`
  a.style.height = `${rect.height}px`
  a.addEventListener('click', (e) => {
    e.preventDefault()
    e.stopPropagation()
    openExternalLink(url)
  })
  overlay.appendChild(a)
}

async function writeTextToClipboard(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }

  const textarea = document.createElement('textarea')
  textarea.value = text
  textarea.setAttribute('readonly', '')
  textarea.style.position = 'fixed'
  textarea.style.left = '-9999px'
  textarea.style.top = '0'
  document.body.appendChild(textarea)
  textarea.select()
  document.execCommand('copy')
  document.body.removeChild(textarea)
}

async function copyHighlightText(hlId: string) {
  const text = resolveHighlightGroup(hlId)?.displayText?.trim()
  hlColorPopup.value = null
  if (!text) return

  try {
    await writeTextToClipboard(text)
  } catch (e) {
    console.error('Copy highlight text failed:', e)
  }
}

// ── fulltext extraction (pdfjs text → OCR fallback) ──────────────────────────
const ocrProgress = ref<{ page: number; total: number } | null>(null)

async function extractFulltextIfNeeded(doc: PDFDocumentProxy, slug: string) {
  try {
    const status = await invoke<{ text_extracted: boolean }>('get_paper_status', { slug })
    if (status.text_extracted) return
  } catch { return }

  // Stage 1: pdfjs embedded text
  try {
    const parts: string[] = []
    for (let i = 1; i <= doc.numPages; i++) {
      const page = await doc.getPage(i)
      const tc = await page.getTextContent()
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const pageText = (tc.items as any[]).map((item: any) => item.str ?? '').join(' ')
      parts.push(pageText)
      page.cleanup()
    }
    const fullText = parts.join('\n\n')
    if (fullText.trim().length > 200) {
      await invoke('save_pdfjs_fulltext', { slug, text: fullText })
      return
    }
  } catch { return }

  // Stage 2: OCR — render each page to canvas, send JPEG to backend
  try {
    const PAGE_SCALE = 2.0
    const pageTexts: string[] = []

    for (let i = 1; i <= doc.numPages; i++) {
      ocrProgress.value = { page: i, total: doc.numPages }

      const page = await doc.getPage(i)
      const viewport = page.getViewport({ scale: PAGE_SCALE })
      const canvas = document.createElement('canvas')
      canvas.width = viewport.width
      canvas.height = viewport.height
      await page.render({ canvas, viewport }).promise
      page.cleanup()

      const dataUrl = canvas.toDataURL('image/jpeg', 0.85)
      const base64 = dataUrl.split(',')[1]

      try {
        const text = await invoke<string>('ocr_page_base64', { pageBase64: base64 })
        pageTexts.push(text)
      } catch {
        pageTexts.push('')
      }
    }

    ocrProgress.value = null

    const combined = pageTexts.join('\n\n')
    if (combined.trim().length > 50) {
      await invoke('save_pdfjs_fulltext', { slug, text: combined })
    }
  } catch {
    ocrProgress.value = null
  }
}

// ── Section (chapter) auto-detection ─────────────────────────────────────────
async function ensureSectionsComputed(doc: PDFDocumentProxy, slug: string) {
  try {
    // Skip if we already have a stored index (any source, including a prior AI run).
    const existing = await invoke<PaperSections | null>('get_sections', { slug })
    if (existing && existing.sections.length) return

    const result = await computeSections(doc)
    if (!result || !result.sections.length) return

    await invoke('save_sections', { slug, data: result })
    window.dispatchEvent(new CustomEvent('argus-sections-updated', { detail: { slug } }))
  } catch (e) {
    console.error('Section detection failed:', e)
  }
}

// ── Load PDF ──────────────────────────────────────────────────────────────────
async function loadPdf() {
  loading.value = true
  error.value = null
  const slug = props.slug
  if (!slug) return
  // Restore the saved zoom if this paper was opened before; otherwise leave the
  // default and fit-to-width once page sizes are known (below). Captured into a
  // local so the async `scale` save-watcher can't clobber the "never opened"
  // signal before we check it — writing a temporary scale here used to persist
  // it mid-load, which permanently suppressed the first-open fit-to-width.
  const savedZoom = loadSavedZoom(slug)
  if (savedZoom !== null) scale.value = savedZoom

  // Load highlights and reading state
  try {
    const [hls, rs] = await Promise.all([
      invoke<Highlight[]>('get_highlights', { slug }),
      invoke<{ page: number; scroll_ratio: number; updated_at: string } | null>('get_reading_state', { slug }),
    ])
    reader.setHighlights(slug, hls)
    reader.setReadingState(slug, rs)
  } catch (e) {
    console.error('Failed to load highlights/state:', e)
  }

  // Load PDF bytes
  let bytes: number[]
  try {
    bytes = await invoke<number[]>('read_pdf_bytes', { slug })
  } catch (e) {
    const msg = String(e)
    // Stale tab: file was deleted (e.g. incomplete import cleaned up on startup)
    if (msg.includes('os error 2') || msg.includes('No such file')) {
      reader.closeTab(slug)
      return
    }
    error.value = msg
    loading.value = false
    return
  }

  try {
    const uint8 = new Uint8Array(bytes)
    // Type 3 fonts render blank in pdf.js; those PDFs use the PDFium raster path.
    rasterFallback.value = pdfUsesType3(uint8)
    // ONE SWITCH. Set it to false to get exactly the pixels the previous release drew.
    // true  = pdf.js keeps the page canvases GPU-backed (without it pdf.js asks for
    //         `willReadFrequently`, i.e. software canvases, and putting a finished 7 MP page on
    //         screen costs a 90-370 ms main-thread composite). Measured in WKWebView at
    //         devicePixelRatio 2, real viewer, 10-12 page fling: frames over 33 ms went 20 -> 2
    //         (plain text paper), 23 -> 2 and 26 -> 10 (figure-heavy), 15 -> 1 (text + figures);
    //         p95 frame 43-55 ms -> 18-27 ms; the render itself is no faster. Peak memory during
    //         the fling is about 220 MB higher (WebContent + GPU, golkar); at rest it is the same.
    // false = software canvases: bit-identical to the previous release (0 differing pixels on all
    //         12 pages compared, canvas size and 1:1 display unchanged either way).
    // With true the bitmap is still cssW x dpr by cssH x dpr, shown 1:1, but the rasteriser is a
    // different one, so anti-aliasing differs a little. Measured against false at 189 %, dpr 2:
    // text-only pages differ in up to 0.4 % of pixels, by at most 2 of 255 levels; pages with
    // figures or tables differ in 0.5-3.4 % of pixels, at the edges of vector lines and fills, by up
    // to ~120 levels (glyph edges over a coloured fill by up to ~40-60).
    const PDF_ENABLE_HWA = true
    const loadingTask = pdfjsLib.getDocument({
      data: uint8,
      isOffscreenCanvasSupported: false,
      enableHWA: PDF_ENABLE_HWA,
      // Without these, non-embedded standard fonts (Helvetica/Times/Symbol — the
      // ones figure labels and diagrams commonly use) and CID fonts render as
      // BLANK: the shapes draw but their text is silently dropped, while embedded
      // (e.g. LaTeX) body text still renders. The pdf.js font/cmap data is copied
      // into /pdfjs by scripts/setup-pdfjs-assets.js (postinstall).
      standardFontDataUrl: '/pdfjs/standard_fonts/',
      cMapUrl: '/pdfjs/cmaps/',
      cMapPacked: true,
    })
    const doc = await loadingTask.promise
    pdfDoc.value = doc
    pageTextSources.clear() // text of a previous document must never feed this one's text layers
    reader.setPdfDoc(doc, slug)
    linkService.setDocument(doc)
    pageCount.value = doc.numPages

    // Pre-fetch all page sizes at scale=1 (fast — no rendering)
    const sizes: { width: number; height: number }[] = []
    for (let i = 1; i <= doc.numPages; i++) {
      const page = await doc.getPage(i)
      const vp = page.getViewport({ scale: 1 })
      sizes.push({ width: vp.width, height: vp.height })
      page.cleanup()
    }
    pageSizes.value = sizes
    pageRefs.value = new Array(sizes.length).fill(null)

    loading.value = false
    await nextTick()

    // First time opening this paper: fit to the page width. Subsequent opens
    // restore the saved zoom above. fitWidth() updates `scale`, which the
    // save-watcher then persists for next time. If the tab is backgrounded now
    // (0 width), the flag makes it fit once it's shown.
    if (savedZoom === null) {
      needsInitialFit.value = true
      fitWidth()
    }

    setupRenderScheduling()
    await restorePosition()
    triggerInitialRender()
    updateScrollThumbs()

    // A page jump requested before this viewer finished mounting (e.g. opened
    // from the sections outline or a snippet) is missed by the reactive watch,
    // which only fires on later changes — apply any pending jump now.
    if (reader.pendingPageJump != null && isActiveTab.value) {
      const target = reader.pendingPageJump
      reader.pendingPageJump = null
      await ensurePageRendered(target - 1)
      scrollToPageIndex(target - 1, 0)
    }

    // Auto-extract fulltext via pdfjs if lopdf/pdftotext extraction previously failed
    extractFulltextIfNeeded(doc, slug)

    // Auto-detect chapter structure (embedded outline → heading heuristic).
    // The AI fallback is never triggered here — it stays a manual action.
    ensureSectionsComputed(doc, slug)
    // Off the critical path: only a cross-page selection ever needs it.
    window.setTimeout(() => { void prefetchBodyFontSize(doc) }, 1500)

    // Auto-update reading status: unread → reading when PDF is opened
    const entry = library.papers.find(p => p.slug === slug)
    if (entry?.reading_status === 'unread') {
      try {
        await invoke('set_reading_status', { slug, status: 'reading' })
        entry.reading_status = 'reading'
      } catch {
        // non-fatal
      }
    }
  } catch (e) {
    error.value = `PDF parse error: ${e}`
    loading.value = false
  }
}

// ── Render scheduling ─────────────────────────────────────────────────────────
// utils/pageRenderPolicy decides what to render, in what order and what to throw away; this
// section carries the plan out. There is deliberately no IntersectionObserver any more: it
// reports every page of a hidden tab as "left the viewport" (which threw every rendered page
// away on each tab switch, so each switch back started from white pages), says nothing about
// direction, distance or priority, and its fixed 600 px margin - less than one page at 189 %
// - could only start a page when it was about to be seen. The plan is computed from the scroll
// position instead (as pdf.js's own viewer does), and a hidden viewer simply stops planning.

const PAGE_GAP = 12 // `.pdf-pages` gap
// pdf.js does its parsing on one worker and the PDFium backend renders one page at a time, and
// each render paints on the main thread, which is what scrolling competes with. Measured with 1,
// 2 and 3 in flight (summed blank time of a series of jumps, ms: golkar 644/667/670, feng
// 450/466/548, soiffer 1334/1340/1361; 6000 px/s flings alike): no real difference, so 2 stays -
// it lets a cheap page overtake a heavy one.
const MAX_CONCURRENT_RENDERS = 2
// A page that has just been asked for stays protected from eviction this long (the jump has
// landed on it by then); a page somebody still WAITS for is protected for as long as they wait.
const PIN_MS = 4000
const WAIT_GIVE_UP_MS = 20_000

/** Every page wrapper's box in scroll-content coordinates, mirroring the CSS (3 px padding, 12 px gap, rounded heights). */
const pageGeometry = computed<PageGeometry>(() => {
  const s = scale.value
  const sizes = pageSizes.value
  const tops = new Float64Array(sizes.length)
  const heights = new Float64Array(sizes.length)
  let y = PDF_PAGE_MARGIN
  for (let i = 0; i < sizes.length; i++) {
    const h = Math.round(sizes[i].height * s)
    tops[i] = y
    heights[i] = h
    y += h + PAGE_GAP
  }
  return { tops, heights }
})

/** Is the page's DOM built for the current scale and devicePixelRatio? */
function isPageFresh(idx: number): boolean {
  return renderedPages.value.has(idx) && pageRenderScales.get(idx) === scale.value
    && pageRenderDprs.get(idx) === (window.devicePixelRatio || 1)
}

/** On screen: the viewer is the active tab AND actually has a box (an ancestor may be display:none). */
function isViewerShown(): boolean {
  const c = containerRef.value
  return !!c && isActiveTab.value && c.clientHeight > 0
}

/** Bitmap pixels a page holds once rendered at the current scale and display density. */
function pagePixelsFor(idx: number): number {
  const held = pagePixelsHeld.get(idx)
  if (held !== undefined) return held
  const sz = pageSizes.value[idx]
  if (!sz) return 0
  const dpr = window.devicePixelRatio || 1
  const w = Math.max(1, Math.round(Math.round(sz.width * scale.value) * dpr))
  const h = Math.max(1, Math.round(Math.round(sz.height * scale.value) * dpr))
  return w * h
}

function pinPage(idx: number) {
  pinnedUntil.set(idx, performance.now() + PIN_MS)
}

function isPinned(idx: number, now: number): boolean {
  return waiters.has(idx) || (pinnedUntil.get(idx) ?? 0) > now
}

function activePins(now: number): Set<number> {
  const out = new Set<number>(waiters.keys())
  for (const [i, until] of pinnedUntil) {
    if (until > now) out.add(i)
    else pinnedUntil.delete(i)
  }
  return out
}

function setupRenderScheduling() {
  const c = containerRef.value
  if (!c) return
  containerResizeObserver?.disconnect()
  // A resize changes the viewport (and so the zone); the 0 -> N resize of a tab being shown
  // is the other thing it catches, and plans for it.
  containerResizeObserver = new ResizeObserver(() => scheduleReconcile())
  containerResizeObserver.observe(c)
  trackedScrollTop = c.scrollTop
}

function teardownRenderScheduling() {
  renderingTornDown = true
  containerResizeObserver?.disconnect()
  containerResizeObserver = null
  if (reconcileRaf) cancelAnimationFrame(reconcileRaf)
  reconcileRaf = 0
  if (scrollIdleTimer) clearTimeout(scrollIdleTimer)
  scrollIdleTimer = null
  wantQueue.length = 0
  for (const idx of [...startedRenders]) abortRender(idx, true)
  pageRenderTasks.forEach((task) => { try { task.cancel() } catch { /* already done */ } })
  pageRenderTasks.clear()
  hiddenViewers.delete(viewerKey)
  for (const url of pageObjectUrls.values()) URL.revokeObjectURL(url)
  pageObjectUrls.clear()
  settleAllWaiters()
}

/** Called for every scroll event: feeds the direction / speed estimate and asks for a plan. */
function noteScroll() {
  const c = containerRef.value
  // A hidden container reports scrollTop 0; that is not where the reader is.
  if (!c || c.clientHeight === 0) return
  trackedScrollTop = c.scrollTop
  if (!isActiveTab.value) return
  scrollTracker.update(c.scrollTop, performance.now(), c.clientHeight)
  scheduleReconcile()
  // Once the scroll has been still for a moment the speed is 0 and the zone settles; plan again.
  if (scrollIdleTimer) clearTimeout(scrollIdleTimer)
  scrollIdleTimer = setTimeout(() => { scrollIdleTimer = null; scheduleReconcile() }, 180)
}

/** Plan at most once per frame, after the frame's scroll events and before it paints. */
function scheduleReconcile() {
  if (reconcileRaf || renderingTornDown) return
  reconcileRaf = requestAnimationFrame(() => { reconcileRaf = 0; reconcile() })
}

function reconcile() {
  const c = containerRef.value
  if (!c || !pdfDoc.value || pageSizes.value.length === 0 || renderingTornDown) return
  // A hidden viewer plans nothing: its pages are exactly what the reader will come back to.
  if (!isViewerShown()) return
  const now = performance.now()
  lastViewportHeight = c.clientHeight
  const stale = new Set<number>()
  for (const i of renderedPages.value) if (!isPageFresh(i)) stale.add(i)
  const plan = planPageRender({
    geometry: pageGeometry.value,
    scrollTop: c.scrollTop,
    viewportHeight: c.clientHeight,
    direction: scrollTracker.direction,
    speed: scrollTracker.speed(now),
    rendered: renderedPages.value,
    stale,
    inFlight: startedRenders,
    queued: new Set(wantQueue),
    pinned: activePins(now),
    pagePixels: pagePixelsFor,
    budgetPixels: ACTIVE_BUDGET_PIXELS,
  })
  // Running work first (so a page both cancelled and evicted is torn down once), then the
  // pages to throw away. Queued work that left the render zone is dropped by rebuilding the
  // queue from the plan below.
  for (const i of plan.cancelInFlight) abortRender(i, true)
  for (const i of plan.evict) {
    // The policy keeps running renders out of `evict`. Were one to slip through, emptying its
    // wrapper now would let the render finish into it and mark a blank page fresh: stop it
    // instead (it takes its own page down when it unwinds, or the next plan evicts it).
    if (renderingPages.has(i)) { abortRender(i, true); continue }
    unrenderPage(i)
  }
  // A page that failed twice is left alone while it stays near; scrolling away from it earns it
  // a fresh start (the old observer retried every time a page re-entered its margin).
  const z = plan.zone
  for (const i of [...failedRenders.keys()]) if (!z || i < z.first || i > z.last) failedRenders.delete(i)
  wantQueue.length = 0
  for (const i of plan.render) if ((failedRenders.get(i) ?? 0) < 2) wantQueue.push(i)
  // The parsed pages (and their decoded images) are kept for the pages on screen and their
  // neighbours only: that is what a zoom re-renders first. Keeping them at all costs about
  // 130 MB of peak memory in a 12-page fling of a figure-heavy paper (measured), so nothing is
  // held for pages nobody is waiting on.
  const vis = plan.visible
  releaseOpListsOutside(vis.length ? { first: vis[0] - 1, last: vis[vis.length - 1] + 1 } : null)
  pumpRenderQueue()
}

function pumpRenderQueue() {
  if (renderingTornDown || !pdfDoc.value || !isViewerShown()) return
  while (wantQueue.length) {
    const idx = wantQueue[0]
    // A page somebody waits on may take one slot more than the cap, so it never queues
    // behind prefetching.
    const cap = MAX_CONCURRENT_RENDERS + (isPinned(idx, performance.now()) ? 1 : 0)
    if (startedRenders.size >= cap) break
    wantQueue.shift()
    // Already up to date (settle whoever waited), or an earlier render of it is still unwinding
    // (its settling replans, and this page is wanted again).
    if (isPageFresh(idx)) { settleWaiter(idx); continue }
    if (startedRenders.has(idx) || renderingPages.has(idx)) continue
    startRender(idx)
  }
}

function settleWaiter(idx: number) {
  const w = waiters.get(idx)
  if (!w) return
  clearTimeout(w.giveUp)
  waiters.delete(idx)
  w.resolve()
}

function settleAllWaiters() {
  for (const idx of [...waiters.keys()]) settleWaiter(idx)
}

function startRender(idx: number) {
  startedRenders.add(idx)
  const p = renderPage(idx)
  inflightRenders.set(idx, p)
  void p.finally(() => {
    if (inflightRenders.get(idx) === p) inflightRenders.delete(idx)
    startedRenders.delete(idx)
    const outcome = renderOutcome.get(idx)
    renderOutcome.delete(idx)
    // A page that keeps failing is left alone instead of being retried every frame; a cancelled
    // one is simply wanted again if the plan still wants it.
    if (outcome === 'ok') failedRenders.delete(idx)
    else if (outcome !== 'aborted') failedRenders.set(idx, (failedRenders.get(idx) ?? 0) + 1)
    if (isPageFresh(idx) || (failedRenders.get(idx) ?? 0) >= 2 || renderingTornDown) settleWaiter(idx)
    pumpRenderQueue()
    scheduleReconcile()
  })
}

/** Stop a render that is queued or running. `discard` also removes what it already put on the page. */
function abortRender(idx: number, discard: boolean) {
  const ctl = renderCtls.get(idx)
  if (!ctl) return
  ctl.cancelled = true
  if (discard) ctl.discard = true
  try { pageRenderTasks.get(idx)?.cancel() } catch { /* already done */ }
}

/** pdf.js keeps a page's parsed operator list until told to let go; free it for pages that are not near. */
function releaseOpList(idx: number) {
  if (!opListPages.delete(idx)) return
  const doc = pdfDoc.value
  if (!doc) return
  void doc.getPage(idx + 1).then(p => p.cleanup()).catch(() => { /* document went away */ })
}

function releaseOpListsOutside(zone: { first: number; last: number } | null) {
  for (const i of [...opListPages]) if (!zone || i < zone.first || i > zone.last) releaseOpList(i)
}

function releasePageObjectUrl(idx: number) {
  const url = pageObjectUrls.get(idx)
  if (url) { URL.revokeObjectURL(url); pageObjectUrls.delete(idx) }
}

// ── Showing and hiding ────────────────────────────────────────────────────────
// A background tab is display:none. That must not cost it a single rendered page: what it
// holds is exactly what the reader sees again on return. So hiding pauses planning (and cancels
// the few renders still running - their text-layer geometry cannot be measured while hidden),
// and showing plans again from the restored scroll position, which finds every page that is
// still rendered at the current scale and density already done.

function onViewerHidden() {
  if (!pdfDoc.value || renderingTornDown) return
  if (reconcileRaf) { cancelAnimationFrame(reconcileRaf); reconcileRaf = 0 }
  if (scrollIdleTimer) { clearTimeout(scrollIdleTimer); scrollIdleTimer = null }
  wantQueue.length = 0
  pinnedUntil.clear()
  // Whoever waited for a page was going to scroll this viewer; it is not on screen any more.
  settleAllWaiters()
  for (const idx of [...startedRenders]) abortRender(idx, true)
  hiddenViewers.set(viewerKey, {
    hiddenSince: performance.now(),
    level: () => hiddenLevel,
    pixelsNow: () => heldPixels(null),
    pixelsAtLevel: () => [1, 2, 3].map(l => heldPixels(hiddenKeepFor(l as HiddenLevel))) as [number, number, number],
    trimTo: trimHidden,
  })
  trimHidden(1)
  enforceHiddenBudget()
}

function onViewerShown() {
  const c = containerRef.value
  if (!c || !pdfDoc.value || renderingTornDown) return
  hiddenViewers.delete(viewerKey)
  hiddenLevel = 0
  trackedScrollTop = c.scrollTop
  scrollTracker.rebase()
  reconcile()
}

function hiddenKeepFor(level: HiddenLevel): Set<number> {
  return hiddenKeepSet(pageGeometry.value, _savedScrollTop ?? trackedScrollTop, lastViewportHeight, level)
}

/** Bitmap pixels held by the rendered pages in `keep` (all of them when null). */
function heldPixels(keep: Set<number> | null): number {
  let sum = 0
  for (const i of renderedPages.value) if (!keep || keep.has(i)) sum += pagePixelsFor(i)
  return sum
}

function trimHidden(level: HiddenLevel) {
  hiddenLevel = level
  // A background tab keeps bitmaps (they are what makes switching back instant), not parsed pages.
  releaseOpListsOutside(null)
  const keep = hiddenKeepFor(level)
  for (const i of [...renderedPages.value]) if (!keep.has(i)) unrenderPage(i)
}

function observePage(el: HTMLDivElement | null, idx: number) {
  pageRefs.value[idx] = el
}

// ── Render / Unrender pages ────────────────────────────────────────────────────

/** The PNG's bytes from whatever the `render_page_image` command handed back (an ArrayBuffer in Tauri v2). */
function rasterBytes(raw: unknown): BlobPart | null {
  if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw)) return raw as BlobPart
  if (Array.isArray(raw)) return new Uint8Array(raw)
  return null
}

/** Did the invoke fail because the backend does not have that command (a build older than the frontend)? */
function isCommandMissing(e: unknown, command: string): boolean {
  const msg = String(e)
  return msg.includes(command) && /not found|unknown command|no such command/i.test(msg)
}

/**
 * One page rasterised by the backend's PDFium, at EXACTLY `pxW` x `pxH` pixels (the page's box x
 * devicePixelRatio, so the browser never resamples it). The PNG comes back as raw bytes and is shown
 * through an object URL; the old command base64-encoded it, which cost a third more to move and a
 * large string to build and parse at each end. A backend that does not have the binary command
 * yet is noticed once and the base64 one used from then on.
 */
async function fetchRasterPage(
  idx: number, scenScale: number, dpr: number, pxW: number, pxH: number,
): Promise<{ src: string; objectUrl: string | null }> {
  if (rasterBinaryAvailable === null && rasterProbe) await rasterProbe
  if (rasterBinaryAvailable !== false) {
    try {
      const call = invoke<unknown>('render_page_image', { slug: props.slug, page: idx + 1, width: pxW, height: pxH })
      if (rasterBinaryAvailable === null && !rasterProbe) {
        rasterProbe = call.then(() => undefined, () => undefined).finally(() => { rasterProbe = null })
      }
      const raw = await call
      const bytes = rasterBytes(raw)
      if (bytes) {
        rasterBinaryAvailable = true
        const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }))
        return { src: url, objectUrl: url }
      }
      rasterBinaryAvailable = false // a payload nobody can use: stop asking
    } catch (e) {
      if (!isCommandMissing(e, 'render_page_image')) throw e
      rasterBinaryAvailable = false
    }
  }
  // Exact pixel size, not a DPI: a whole-number DPI can't hit the box exactly.
  const b64 = await invoke<string>('render_page_png', {
    slug: props.slug, page: idx + 1, dpi: Math.round(72 * scenScale * dpr),
    width: pxW, height: pxH,
  })
  return { src: `data:image/png;base64,${b64}`, objectUrl: null }
}

async function renderPage(idx: number) {
  if (!pdfDoc.value) { renderOutcome.set(idx, 'aborted'); return }
  // Already up to date? Content rendered at a different scale is still on screen
  // but stale, so it has to fall through and re-render.
  if (isPageFresh(idx)) { renderOutcome.set(idx, 'ok'); return }
  if (renderingPages.has(idx)) { renderOutcome.set(idx, 'aborted'); return }
  renderingPages.add(idx)

  const el = pageRefs.value[idx]
  if (!el) { renderingPages.delete(idx); renderOutcome.set(idx, 'failed'); return }

  const myGen = renderGeneration
  const ctl: RenderCtl = { cancelled: false, discard: false }
  renderCtls.set(idx, ctl)
  // A newer zoom owns the page, or the scheduler cancelled this render (the page left the
  // keep zone, the tab was hidden or closed).
  const superseded = () => myGen !== renderGeneration || ctl.cancelled
  const scenScale = scale.value
  // Content from a previous scale. It stays on screen until the new canvas is
  // painted — swapping only at that point is what removes the white flash.
  const stale = Array.from(el.children) as HTMLElement[]
  // Track every node this render appends so we can tear down a half-built page
  // if a newer generation supersedes us at any await point (avoids ghost layers).
  const appended: HTMLElement[] = []
  // Once the canvas has replaced the previous content it must survive being
  // superseded — tearing it down too would leave a genuinely blank page, which
  // is the very thing this rework exists to prevent. It stays, gets resized to
  // whatever scale is now current, and the next render replaces it properly.
  // (A render the scheduler discards takes even that down: the page is being evicted.)
  let committedCanvas: HTMLElement | null = null
  // The object URL of a rasterised page's image, until the image is in the DOM and owns it.
  let newObjectUrl: string | null = null
  const cleanupAppended = () => {
    appended.forEach(n => { if (n !== committedCanvas || ctl.discard) n.remove() })
    if (newObjectUrl) { URL.revokeObjectURL(newObjectUrl); newObjectUrl = null }
    if (committedCanvas && !ctl.discard) rescalePageDom(idx, scale.value)
  }
  const bail = () => { cleanupAppended(); renderOutcome.set(idx, 'aborted') }
  try {
    const page: PDFPageProxy = await pdfDoc.value.getPage(idx + 1)
    // Scale changed while we were fetching the page — abandon.
    if (superseded()) { bail(); return }
    const dpr = window.devicePixelRatio || 1
    // Logical viewport for CSS layout / text layer / highlights
    const logicalVp = page.getViewport({ scale: scenScale })
    // The page's box on screen in CSS px — the same rounding as the page wrapper in
    // the template — and the bitmap that has to cover it pixel for pixel. The bitmap
    // is derived FROM the box: rounding `width × dpr` on its own (and asking PDFium
    // for a whole-number DPI) left it a pixel or a few off the box at almost every
    // zoom, and the browser then resampled the entire page to fit, smearing every
    // glyph edge across two device pixels — a uniformly soft page.
    const cssW = Math.round(logicalVp.width)
    const cssH = Math.round(logicalVp.height)
    const pxW = Math.max(1, Math.round(cssW * dpr))
    const pxH = Math.max(1, Math.round(cssH * dpr))

    // The page's visual layer. Normally pdf.js renders vector-crisp to a canvas.
    // But some PDFs use fonts pdf.js can't render (Type 3 figure fonts → the text
    // renders blank); for those we rasterise the page with the bundled PDFium
    // engine (the same one the AI page-view uses) and show it as an <img>, which
    // also dodges WebKit's canvas size cap. Either way the text/highlight/
    // annotation layers below are pdf.js's, so selection and highlights still work.
    let contentEl: HTMLElement | null = null
    if (rasterFallback.value) {
      try {
        const { src, objectUrl } = await fetchRasterPage(idx, scenScale, dpr, pxW, pxH)
        newObjectUrl = objectUrl
        if (superseded()) { bail(); return }
        const img = new Image()
        img.className = 'pdf-canvas'
        img.src = src
        try { await img.decode() } catch { /* show it anyway */ }
        img.style.width = `${cssW}px`
        img.style.height = `${cssH}px`
        contentEl = img
      } catch (e) {
        console.error(`render_page_png(${idx}) failed; falling back to pdf.js:`, e)
        if (newObjectUrl) { URL.revokeObjectURL(newObjectUrl); newObjectUrl = null }
        // fall through to the pdf.js canvas path below
      }
    }
    if (!contentEl) {
      // Physical viewport for crisp canvas rendering on HiDPI screens. Rendered
      // DETACHED: an empty canvas in the page would cover the old content with a
      // white rectangle for the whole render (the flash we're avoiding). It joins
      // the DOM only once it actually has the page on it.
      //
      // (Showing a fresh page's canvas while pdf.js is still painting it, so text appears before
      // the figures, was tried and dropped. Measured in WKWebView (jumps to unrendered pages, three
      // runs each) it shortened the mean blank by 24 ms on one figure-heavy paper (111 against 135
      // ms) and 38 ms on another (59 against 97 ms), and nothing for an ordinary 20-100 ms page. It
      // costs a half-painted canvas that must be removed whenever the render is cancelled, and a
      // page that shows text it cannot select yet.)
      const canvas = document.createElement('canvas')
      canvas.className = 'pdf-canvas'
      canvas.width = pxW
      canvas.height = pxH
      canvas.style.width = `${cssW}px`
      canvas.style.height = `${cssH}px`

      // Draw the page to fill the bitmap exactly — what pdf.js's own viewer does: the
      // logical viewport plus an output transform with a separate factor per axis.
      const task = page.render({
        canvas,
        viewport: logicalVp,
        transform: [pxW / logicalVp.width, 0, 0, pxH / logicalVp.height, 0, 0],
      })
      pageRenderTasks.set(idx, task)
      await task.promise
      pageRenderTasks.delete(idx)
      contentEl = canvas
    }

    // Scale changed during render — a newer generation owns the page now, so
    // bail without touching what's on screen.
    if (superseded()) { bail(); return }

    // The swap: new content in, previous scale's content out, same frame.
    el.appendChild(contentEl)
    appended.push(contentEl)
    committedCanvas = contentEl
    stale.forEach(n => n.remove())
    // The previous image (if any) is gone from the DOM, so its object URL can go too.
    releasePageObjectUrl(idx)
    if (newObjectUrl) { pageObjectUrls.set(idx, newObjectUrl); newObjectUrl = null }

    // Text layer at logical scale so CSS positions match layout
    const textLayerDiv = document.createElement('div')
    textLayerDiv.className = 'textLayer'
    // pdfjs v5 uses --total-scale-factor to size the container via setLayerDimensions
    textLayerDiv.style.setProperty('--total-scale-factor', String(scenScale))
    el.appendChild(textLayerDiv)
    appended.push(textLayerDiv)

    // Kept across re-renders: the text does not depend on the scale.
    let textSource = pageTextSources.get(idx) ?? null
    try {
      if (!textSource) {
        // A TextContent object rather than page.streamTextContent(): the TextLayer accepts either
        // and builds the same spans, but the object is also what the furniture planner reads.
        textSource = {
          content: (await page.getTextContent()) as unknown as TextContentLike,
          view: Array.from(page.view),
          rotate: page.rotate,
        }
      }
      const textLayer = new pdfjsLib.TextLayer({
        textContentSource: textSource.content as unknown as ConstructorParameters<typeof pdfjsLib.TextLayer>[0]['textContentSource'],
        container: textLayerDiv,
        viewport: logicalVp,
      })
      await textLayer.render()
      // Anchor drag-selection so dragging into whitespace doesn't over-select.
      setupTextLayerSelection(textLayerDiv)
    } catch (e) {
      console.warn('TextLayer render failed:', e)
    }
    if (superseded()) { bail(); return }

    // Highlight overlay at logical scale
    const hlDiv = document.createElement('div')
    hlDiv.className = 'highlight-overlay'
    hlDiv.style.width = `${cssW}px`
    hlDiv.style.height = `${cssH}px`
    el.appendChild(hlDiv)
    appended.push(hlDiv)

    renderHighlightsOnPage(hlDiv, idx)

    // Annotation layer (links, forms) — rendered on top so links are clickable
    const annotationLayerDiv = document.createElement('div')
    annotationLayerDiv.className = 'annotationLayer'
    // Same as the text layer: pdfjs sizes this container via setLayerDimensions,
    // which writes `width: calc(var(--total-scale-factor) * <pageWidth>px)`. Without
    // this variable the calc is invalid-at-computed-value-time, so the layer
    // collapses to auto/0 and its percent-positioned link boxes shrink to nothing —
    // links render but can't be clicked. Set it before render so links get real size.
    annotationLayerDiv.style.setProperty('--total-scale-factor', String(scenScale))
    el.appendChild(annotationLayerDiv)
    appended.push(annotationLayerDiv)

    try {
      const annotations = await page.getAnnotations()
      if (annotations.length > 0) {
        const annotationLayer = new pdfjsLib.AnnotationLayer({
          div: annotationLayerDiv,
          page,
          viewport: logicalVp,
          linkService,
        } as any)
        await annotationLayer.render({
          viewport: logicalVp,
          annotations,
          page,
          linkService,
          renderForms: false,
        } as any)
      }
    } catch (e) {
      console.warn('AnnotationLayer render failed:', e)
    }
    // Linkify below measures the text layer's boxes, which only exist while the page is laid
    // out: a tab hidden since the checks above would get overlays of zero size.
    if (superseded()) { bail(); return }

    annotationLayerDiv.addEventListener('click', onAnnotationLayerClick)

    // Linkify plain-text URLs in the text layer (many PDFs render URLs as text)
    const linkifyDiv = document.createElement('div')
    linkifyDiv.className = 'linkify-overlay'
    linkifyDiv.style.width = `${cssW}px`
    linkifyDiv.style.height = `${cssH}px`
    el.appendChild(linkifyDiv)
    appended.push(linkifyDiv)
    try {
      linkifyTextLayer(textLayerDiv, linkifyDiv, annotationLayerDiv)
    } catch (e) {
      console.warn('Linkify text layer failed:', e)
    }

    if (textSource) pageTextSources.set(idx, textSource)
    renderedPages.value = new Set(renderedPages.value).add(idx)
    pageRenderScales.set(idx, scenScale)
    pageRenderDprs.set(idx, dpr)
    pagePixelsHeld.set(idx, pxW * pxH)
    // pdf.js keeps the parsed operator list (and decoded images) of a page until cleanup(). It used
    // to be dropped here, so every re-render - a zoom, a display change - parsed the page again.
    // Measured in WKWebView, re-rendering the two visible pages after one zoom step took 10-20 ms
    // a page instead of 78-91 ms on two figure-heavy papers (golkar 20 against 91 ms, hero 10
    // against 78 ms; feng 97 against 129 ms).
    // It is kept while the page is on screen or next to it and released by releaseOpListsOutside /
    // unrenderPage once it is not.
    opListPages.add(idx)
    renderOutcome.set(idx, 'ok')
    // The window moved to another display while this page was rendering: the plan sees the page
    // as stale now and asks for it again.
    if (dpr !== (window.devicePixelRatio || 1)) scheduleReconcile()
  } catch (e) {
    // RenderingCancelledException is expected when a zoom cancels in-flight work.
    if ((e as { name?: string })?.name !== 'RenderingCancelledException') {
      console.error(`renderPage(${idx}) failed:`, e)
      renderOutcome.set(idx, 'failed')
    } else {
      renderOutcome.set(idx, 'aborted')
    }
  } finally {
    if (newObjectUrl) URL.revokeObjectURL(newObjectUrl)
    // The scheduler discarded this render after it had already replaced the page's content.
    if (ctl.discard && committedCanvas) unrenderPage(idx)
    if (renderCtls.get(idx) === ctl) renderCtls.delete(idx)
    pageRenderTasks.delete(idx)
    renderingPages.delete(idx)
  }
}

function unrenderPage(idx: number) {
  const el = pageRefs.value[idx]
  if (!el) return
  // Keep the placeholder size — only remove rendered children
  while (el.firstChild) el.removeChild(el.firstChild)
  releasePageObjectUrl(idx)
  pageRenderScales.delete(idx)
  pageRenderDprs.delete(idx)
  pagePixelsHeld.delete(idx)
  pageTextSources.delete(idx)
  releaseOpList(idx)
  const next = new Set(renderedPages.value)
  next.delete(idx)
  renderedPages.value = next
}

/**
 * Make a page's already-rendered content match a new scale immediately, without
 * re-rendering: the canvas bitmap is stretched by the browser (soft for a
 * moment) and the pdfjs layers are re-sized through the CSS variable they are
 * built around. The crisp re-render then swaps in underneath the user's notice.
 */
function rescalePageDom(idx: number, newScale: number) {
  const el = pageRefs.value[idx]
  const base = pageSizes.value[idx]
  if (!el || !base) return
  const w = Math.round(base.width * newScale)
  const h = Math.round(base.height * newScale)

  el.querySelectorAll<HTMLElement>('.pdf-canvas, .highlight-overlay, .linkify-overlay')
    .forEach((n) => { n.style.width = `${w}px`; n.style.height = `${h}px` })
  // pdfjs sizes and positions these layers off this variable, which is exactly
  // what it exists for — zooming without re-running the layer.
  el.querySelectorAll<HTMLElement>('.textLayer, .annotationLayer')
    .forEach((n) => { n.style.setProperty('--total-scale-factor', String(newScale)) })

  // Highlight boxes are absolute px at the old scale, so they need redrawing.
  const overlay = el.querySelector('.highlight-overlay') as HTMLDivElement | null
  if (overlay) renderHighlightsOnPage(overlay, idx)
}

// ── Re-render on scale change ─────────────────────────────────────────────────
watch(scale, async (newScale) => {
  saveZoom()
  // Mark every in-flight render stale and cancel the running pdfjs tasks so
  // none of them lands a canvas sized for the previous scale.
  renderGeneration++
  pageRenderTasks.forEach((task) => { try { task.cancel() } catch { /* already done */ } })
  pageRenderTasks.clear()
  // Whatever was queued was planned for the old scale; the plan after the wait below redoes it.
  wantQueue.length = 0
  failedRenders.clear()

  // Resize what's already on screen RIGHT NOW. The page wrappers resize
  // reactively with `scale`, so without this their content would sit at the old
  // size inside a differently-sized box. This used to wipe every page instead
  // and re-render from scratch — that blank gap, across a full re-render of
  // canvas + text + annotation layers, was the white screen.
  pageRefs.value.forEach((_, idx) => rescalePageDom(idx, newScale))

  // Wait for the cancelled/stale renders to unwind so their page locks free up
  // before we re-render — otherwise a re-render could be blocked or doubled.
  // Each render releases its own lock when it settles (renderPage's `finally`), so
  // nothing is cleared here: clearing the set also dropped the lock of a render that
  // had started DURING this wait (the observer, a display change) at the new scale,
  // and the trigger below then rendered the same page a second time beside it —
  // every layer of that page doubled, highlights drawn twice as dark.
  await Promise.allSettled([...inflightRenders.values()])

  await nextTick()
  updateScrollThumbs()
  // Re-render the pages in the render zone at the new scale. `pageRenderScales` is
  // deliberately left alone: it still records the OLD scale, which is what makes the plan
  // treat these pages as stale (re-render those in the zone, drop the rest) and
  // renderPage rebuild them.
  triggerInitialRender()
})

// ── Highlight rendering ───────────────────────────────────────────────────────
function renderHighlightsOnPage(container: HTMLDivElement, pageIndex: number) {
  container.innerHTML = ''
  const hls = pageHighlights(pageIndex)
  const s = scale.value
  hls.forEach(hl => {
    const validRects = hl.rects.filter(r => isFinite(r.x) && isFinite(r.y))
    if (validRects.length === 0) return

    if (hl.style === 'underline') {
      validRects.forEach(rect => {
        const div = document.createElement('div')
        div.className = 'hl-rect'
        div.style.left   = `${rect.x * s}px`
        div.style.top    = `${rect.y * s}px`
        div.style.width  = `${rect.width * s}px`
        div.style.height = `${rect.height * s}px`
        div.style.background = 'transparent'
        div.style.borderBottom = `2px solid ${hl.color}`
        div.dataset.hlId = hl.id
        div.addEventListener('click', (e) => {
          e.stopPropagation()
          openHighlightNote(div.getBoundingClientRect(), hl.id)
        })
        div.addEventListener('contextmenu', (e) => {
          e.preventDefault()
          e.stopPropagation()
          openHighlightMenu(e, hl.id)
        })
        container.appendChild(div)
      })
    } else {
      // Use SVG <g opacity> so overlapping rects within the same highlight
      // are composited as a unit — no alpha stacking between them.
      const NS = 'http://www.w3.org/2000/svg'
      const svg = document.createElementNS(NS, 'svg') as SVGSVGElement
      svg.style.cssText = 'position:absolute;top:0;left:0;width:100%;height:100%;overflow:visible;pointer-events:none'
      svg.dataset.hlId = hl.id

      const g = document.createElementNS(NS, 'g') as SVGGElement
      g.setAttribute('fill', hl.color)
      g.setAttribute('opacity', '0.35')
      g.style.transition = 'opacity 0.15s'

      validRects.forEach(rect => {
        const r = document.createElementNS(NS, 'rect') as SVGRectElement
        r.setAttribute('x', String(rect.x * s))
        r.setAttribute('y', String(rect.y * s))
        r.setAttribute('width', String(rect.width * s))
        r.setAttribute('height', String(rect.height * s))
        r.style.pointerEvents = 'auto'
        r.style.cursor = 'pointer'
        r.addEventListener('click', (e) => {
          e.stopPropagation()
          openHighlightNote(r.getBoundingClientRect(), hl.id)
        })
        r.addEventListener('contextmenu', (e) => {
          e.preventDefault()
          e.stopPropagation()
          openHighlightMenu(e, hl.id)
        })
        g.appendChild(r)
      })

      g.addEventListener('mouseenter', () => { g.setAttribute('opacity', '0.6') })
      g.addEventListener('mouseleave', () => { g.setAttribute('opacity', '0.35') })

      svg.appendChild(g)
      container.appendChild(svg)
    }
  })
  drawReadingHighlight(container, pageIndex)
}

// Re-render highlight overlays when THIS tab's highlights change
watch(() => reader.highlightsFor(props.slug), () => {
  renderedPages.value.forEach(idx => {
    const el = pageRefs.value[idx]
    if (!el) return
    const overlay = el.querySelector('.highlight-overlay') as HTMLDivElement | null
    if (overlay) renderHighlightsOnPage(overlay, idx)
  })
})

// Re-render highlight overlays when scale changes (handled by full page re-render above)

// ── Jump to highlight ─────────────────────────────────────────────────────────
// These commands target the visible tab; ignore them in backgrounded viewers.
watch(() => reader.pendingPageJump, async (page) => {
  if (page === null || !isActiveTab.value) return
  reader.pendingPageJump = null
  const pageIndex = page - 1
  await ensurePageRendered(pageIndex)
  scrollToPageIndex(pageIndex, 0)
})

watch(() => reader.scrollToHighlightId, async (id) => {
  if (!id || !isActiveTab.value) return
  const hl = reader.highlightsFor(props.slug).find(h => h.id === id)
  if (!hl) return
  reader.scrollToHighlightId = null
  // Land on where the highlight starts; a cross-page one flashes on every page of
  // it that is rendered (the later pages may be off-screen and not rendered yet).
  const members = highlightGroupIndex.value.get(id)?.members ?? [hl]
  const first = members[0]
  const pageIndex = first.page - 1
  await ensurePageRendered(pageIndex)
  scrollToPageIndex(pageIndex, first.rects[0]?.y ?? 0)
  // Flash the highlight
  setTimeout(() => {
    for (const m of members) {
      const el = pageRefs.value[m.page - 1]
      const hlEl = el?.querySelector(`[data-hl-id="${m.id}"]`) as HTMLDivElement | null
      if (!hlEl) continue
      hlEl.classList.add('hl-flash')
      setTimeout(() => hlEl.classList.remove('hl-flash'), 1000)
    }
  }, 100)
})

async function ensurePageRendered(pageIndex: number) {
  if (renderedPages.value.has(pageIndex)) return
  await requestRenderPage(pageIndex)
  await nextTick()
}

// ── Progress tracking ─────────────────────────────────────────────────────────
function onScroll() {
  // The scroll event of a tab that has just been hidden arrives after display:none, where
  // scrollTop reads 0: acting on it would show page 1 in the toolbar and save it as the position.
  if (!containerRef.value || containerRef.value.clientHeight === 0) return
  noteScroll()
  updateDisplayPage()
  showScrollThumbs()
  repositionAnchoredPopups()
  if (progressDebounce) clearTimeout(progressDebounce)
  progressDebounce = setTimeout(flushReadingState, 700)
}

// The selection toolbar and highlight popups are `position: fixed` at viewport
// coordinates, anchored to content (a text selection or a highlight). The content
// scrolls 1:1 in the viewport but a fixed popup does not, so without this it ends
// up stranded over unrelated text. Shift every open popup by the scroll delta to
// keep it glued to its anchor; if the anchor scrolls off-screen the popup goes with
// it (no clamping — clamping would re-detach it from the anchor).
let lastPopupScrollTop = 0
let lastPopupScrollLeft = 0
function repositionAnchoredPopups() {
  const el = containerRef.value
  if (!el) return
  const dx = el.scrollLeft - lastPopupScrollLeft
  const dy = el.scrollTop - lastPopupScrollTop
  lastPopupScrollTop = el.scrollTop
  lastPopupScrollLeft = el.scrollLeft
  if (!dx && !dy) return
  if (selectionPopup.value) {
    selectionPopup.value = {
      ...selectionPopup.value,
      x: selectionPopup.value.x - dx,
      y: selectionPopup.value.y - dy,
    }
  }
  if (hlNotePopup.value) {
    hlNotePopup.value = { ...hlNotePopup.value, x: hlNotePopup.value.x - dx, y: hlNotePopup.value.y - dy }
  }
  if (hlColorPopup.value) {
    hlColorPopup.value = { ...hlColorPopup.value, x: hlColorPopup.value.x - dx, y: hlColorPopup.value.y - dy }
  }
}

function measureScrollThumb(clientSize: number, scrollSize: number, scrollOffset: number) {
  const trackSize = Math.max(0, clientSize - SCROLL_THUMB_INSET * 2)
  const maxScroll = scrollSize - clientSize
  if (trackSize <= 0 || maxScroll <= 0) return { visible: false, size: 0, offset: 0 }

  const size = Math.max(SCROLL_THUMB_MIN_SIZE, trackSize * (clientSize / scrollSize))
  const offset = SCROLL_THUMB_INSET + (trackSize - size) * (scrollOffset / maxScroll)
  return { visible: true, size, offset }
}

function updateScrollThumbs() {
  const el = containerRef.value
  if (!el) return
  scrollThumbs.value = {
    vertical: measureScrollThumb(el.clientHeight, el.scrollHeight, el.scrollTop),
    horizontal: measureScrollThumb(el.clientWidth, el.scrollWidth, el.scrollLeft),
  }
}

function showScrollThumbs() {
  updateScrollThumbs()
  const hasScrollableAxis = scrollThumbs.value.vertical.visible || scrollThumbs.value.horizontal.visible
  scrollThumbsActive.value = hasScrollableAxis
  if (scrollThumbHideTimer) clearTimeout(scrollThumbHideTimer)
  // Don't auto-hide while the user is dragging a thumb.
  if (thumbDrag) return
  scrollThumbHideTimer = setTimeout(() => {
    scrollThumbsActive.value = false
  }, SCROLL_THUMB_HIDE_DELAY)
}

// ── Draggable scroll thumbs ───────────────────────────────────────────────────
let thumbDrag: { axis: 'v' | 'h'; startPos: number; startScroll: number; ratio: number } | null = null

function onThumbPointerDown(axis: 'v' | 'h', e: PointerEvent) {
  const el = containerRef.value
  if (!el || e.button !== 0) return
  e.preventDefault()
  e.stopPropagation()
  try { (e.target as HTMLElement).setPointerCapture(e.pointerId) } catch { /* ignore */ }

  // Map a 1px move of the thumb to the corresponding scroll delta.
  let ratio = 0
  if (axis === 'v') {
    const track = el.clientHeight - SCROLL_THUMB_INSET * 2
    const denom = track - scrollThumbs.value.vertical.size
    ratio = denom > 0 ? (el.scrollHeight - el.clientHeight) / denom : 0
    thumbDrag = { axis, startPos: e.clientY, startScroll: el.scrollTop, ratio }
  } else {
    const track = el.clientWidth - SCROLL_THUMB_INSET * 2
    const denom = track - scrollThumbs.value.horizontal.size
    ratio = denom > 0 ? (el.scrollWidth - el.clientWidth) / denom : 0
    thumbDrag = { axis, startPos: e.clientX, startScroll: el.scrollLeft, ratio }
  }
  scrollThumbsActive.value = true
  if (scrollThumbHideTimer) { clearTimeout(scrollThumbHideTimer); scrollThumbHideTimer = null }
  window.addEventListener('pointermove', onThumbPointerMove)
  window.addEventListener('pointerup', onThumbPointerUp)
}

function onThumbPointerMove(e: PointerEvent) {
  const el = containerRef.value
  if (!thumbDrag || !el) return
  if (thumbDrag.axis === 'v') {
    el.scrollTop = thumbDrag.startScroll + (e.clientY - thumbDrag.startPos) * thumbDrag.ratio
  } else {
    el.scrollLeft = thumbDrag.startScroll + (e.clientX - thumbDrag.startPos) * thumbDrag.ratio
  }
  updateScrollThumbs()
  scrollThumbsActive.value = true
}

function onThumbPointerUp() {
  thumbDrag = null
  window.removeEventListener('pointermove', onThumbPointerMove)
  window.removeEventListener('pointerup', onThumbPointerUp)
  showScrollThumbs()
}

function updateDisplayPage() {
  if (!containerRef.value || pageSizes.value.length === 0) return
  const scrollTop = containerRef.value.scrollTop
  let cumY = 0
  const gap = 12
  for (let i = 0; i < pageSizes.value.length; i++) {
    const pageH = pageSizes.value[i].height * scale.value + gap
    if (cumY + pageH > scrollTop + 10) {
      displayPage.value = i + 1
      pageInputValue.value = String(i + 1)
      return
    }
    cumY += pageH
  }
  displayPage.value = pageSizes.value.length
  pageInputValue.value = String(pageSizes.value.length)
}

function flushReadingState() {
  const c = containerRef.value
  if (!c || pageSizes.value.length === 0) return
  // A hidden container reads scrollTop 0, which would be saved as "back on page 1": the one
  // value that survives display:none is the one the scroll events tracked.
  const scrollTop = c.clientHeight > 0 ? c.scrollTop : trackedScrollTop
  const gap = 12
  let cumY = 0
  for (let i = 0; i < pageSizes.value.length; i++) {
    const pageH = pageSizes.value[i].height * scale.value + gap
    if (cumY + pageH > scrollTop + 10 || i === pageSizes.value.length - 1) {
      const ratio = Math.max(0, Math.min(1, (scrollTop - cumY) / pageH))
      // This viewer's own slug: when it is being backgrounded the active tab is already another one.
      reader.persistReadingState({
        page: i + 1,
        scroll_ratio: ratio,
        updated_at: new Date().toISOString(),
      }, props.slug)
      return
    }
    cumY += pageH
  }
}

// ── Restore scroll position ───────────────────────────────────────────────────
async function restorePosition() {
  const rs = reader.readingStateFor(props.slug)
  if (!rs || !containerRef.value) return
  const gap = 12
  let cumY = 0
  for (let i = 0; i < rs.page - 1 && i < pageSizes.value.length; i++) {
    cumY += pageSizes.value[i].height * scale.value + gap
  }
  if (rs.page <= pageSizes.value.length) {
    const pageH = pageSizes.value[rs.page - 1].height * scale.value + gap
    cumY += rs.scroll_ratio * pageH
  }
  containerRef.value.scrollTop = cumY
  trackedScrollTop = cumY
  // Opened while backgrounded: a hidden container has no scroll offset to set, so the position
  // is applied when the tab is first shown (showViewer, after its fit-to-width).
  if (!isActiveTab.value) restorePending = true
  displayPage.value = rs.page
  pageInputValue.value = String(rs.page)
}

// ── Flush on close ────────────────────────────────────────────────────────────
function handleBack() {
  if (progressDebounce) { clearTimeout(progressDebounce); progressDebounce = null }
  flushReadingState()
  reader.closePaper()
}

defineExpose({
  closeToList: handleBack,
})

// ── Zoom ──────────────────────────────────────────────────────────────────────
function zoomIn()  { scale.value = Math.min(4, +(scale.value + 0.25).toFixed(2)) }
function zoomOut() { scale.value = Math.max(0.5, +(scale.value - 0.25).toFixed(2)) }
function fitWidth() {
  if (!containerRef.value || pageSizes.value.length === 0) return
  const containerW = containerRef.value.clientWidth - PDF_PAGE_MARGIN * 2
  // A backgrounded tab is `display:none`, so its width is 0 — defer the fit
  // until the tab is shown (see the isActiveTab watch) rather than snapping to
  // a bogus minimum scale.
  if (containerW <= 0) return
  const pageW = pageSizes.value[0].width
  scale.value = Math.max(0.5, Math.min(4, +(containerW / pageW).toFixed(3)))
  needsInitialFit.value = false
}

// ── Page jump ─────────────────────────────────────────────────────────────────
function onPageInputChange(e: Event) {
  const val = parseInt((e.target as HTMLInputElement).value)
  if (!isNaN(val) && val >= 1 && val <= pageCount.value) jumpToPage(val)
}

function jumpToPage(page: number) {
  if (!containerRef.value || pageSizes.value.length === 0) return
  const gap = 12
  let cumY = 0
  for (let i = 0; i < page - 1 && i < pageSizes.value.length; i++) {
    cumY += pageSizes.value[i].height * scale.value + gap
  }
  containerRef.value.scrollTop = cumY
}

function scrollToPageIndex(pageIndex: number, offsetYAtScale1 = 0) {
  if (!containerRef.value || pageSizes.value.length === 0) return
  const gap = 12
  let cumY = 0
  for (let i = 0; i < pageIndex; i++) {
    cumY += pageSizes.value[i].height * scale.value + gap
  }
  cumY += offsetYAtScale1 * scale.value
  containerRef.value.scrollTop = Math.max(0, cumY - 60)
}

// ── Search ────────────────────────────────────────────────────────────────────

interface SearchMatch { pageIndex: number; rects: Rect[]; matchOnPage: number }

const searchOpen          = ref(false)
const searchQuery         = ref('')
const searchCaseSensitive = ref(false)
const searchWholeWord     = ref(false)
const searchHighlightAll  = ref(true)
const searchMatches       = ref<SearchMatch[]>([])
const searchMatchIndex    = ref(0)
const searchBusy          = ref(false)
const searchInputRef      = ref<HTMLInputElement | null>(null)

const pageTextCache = new Map<number, string>()

async function fetchPageText(pageIndex: number): Promise<string> {
  if (pageTextCache.has(pageIndex)) return pageTextCache.get(pageIndex)!
  if (!pdfDoc.value) return ''
  try {
    const page = await pdfDoc.value.getPage(pageIndex + 1)
    const tc = await page.getTextContent()
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const text = (tc.items as any[]).map((it: any) => it.str ?? '').join('')
    pageTextCache.set(pageIndex, text)
    return text
  } catch { return '' }
}

function buildSearchRegex(): RegExp | null {
  const q = searchQuery.value.trim()
  if (!q) return null
  try {
    const escaped = q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const pattern = searchWholeWord.value ? `\\b${escaped}\\b` : escaped
    return new RegExp(pattern, searchCaseSensitive.value ? 'g' : 'gi')
  } catch { return null }
}

function findAllPageMatchRects(pageIndex: number, regex: RegExp): Rect[][] {
  const el = pageRefs.value[pageIndex]
  if (!el) return []
  const textLayer = el.querySelector('.textLayer')
  if (!textLayer) return []

  const spans = Array.from(textLayer.querySelectorAll('span')).filter(
    s => !s.querySelector('span')
  ) as HTMLSpanElement[]
  const pageRect = el.getBoundingClientRect()

  let fullText = ''
  const map: { span: HTMLSpanElement; start: number; end: number }[] = []
  for (const span of spans) {
    const t = span.textContent ?? ''
    if (!t) continue
    map.push({ span, start: fullText.length, end: fullText.length + t.length })
    fullText += t
  }

  const allMatches: Rect[][] = []
  regex.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = regex.exec(fullText)) !== null) {
    const ms = m.index, me = ms + m[0].length
    const matchRects: Rect[] = []
    for (const { span, start, end } of map) {
      if (end <= ms || start >= me) continue
      const textNode = Array.from(span.childNodes).find(n => n.nodeType === Node.TEXT_NODE)
      if (!textNode) continue
      const rs = Math.max(ms, start) - start
      const re = Math.min(me, end) - start
      if (rs >= re) continue
      try {
        const range = document.createRange()
        range.setStart(textNode, rs)
        range.setEnd(textNode, re)
        for (const cr of range.getClientRects()) {
          if (cr.width > 0 && cr.height > 0) {
            matchRects.push({
              x: (cr.left - pageRect.left) / scale.value,
              y: (cr.top  - pageRect.top)  / scale.value,
              width:  cr.width  / scale.value,
              height: cr.height / scale.value,
            })
          }
        }
      } catch { /* ignore */ }
    }
    if (matchRects.length > 0) allMatches.push(matchRects)
    if (m[0].length === 0) regex.lastIndex++
  }
  return allMatches
}

function refreshSearchOverlays() {
  // Remove stale overlays
  pageRefs.value.forEach(el => { if (el) el.querySelector('.search-overlay')?.remove() })
  if (!searchOpen.value || !searchQuery.value.trim()) return

  const byPage = new Map<number, { match: SearchMatch; idx: number }[]>()
  searchMatches.value.forEach((m, i) => {
    if (!byPage.has(m.pageIndex)) byPage.set(m.pageIndex, [])
    byPage.get(m.pageIndex)!.push({ match: m, idx: i })
  })

  byPage.forEach((entries, pageIndex) => {
    const el = pageRefs.value[pageIndex]
    if (!el || entries.every(e => e.match.rects.length === 0)) return
    const overlay = document.createElement('div')
    overlay.className = 'search-overlay'
    entries.forEach(({ match, idx }) => {
      const isCurrent = idx === searchMatchIndex.value
      if (!searchHighlightAll.value && !isCurrent) return
      match.rects.forEach(rect => {
        const div = document.createElement('div')
        div.style.cssText = `
          position:absolute;
          left:${rect.x * scale.value}px;
          top:${rect.y * scale.value}px;
          width:${rect.width * scale.value}px;
          height:${rect.height * scale.value}px;
          background:${isCurrent ? 'rgba(255,165,0,0.6)' : 'rgba(255,220,0,0.38)'};
          border-radius:2px;pointer-events:none;
        `
        overlay.appendChild(div)
      })
    })
    el.appendChild(overlay)
  })
}

async function runSearch() {
  const q = searchQuery.value.trim()
  if (!q || !pdfDoc.value) {
    searchMatches.value = []; searchMatchIndex.value = 0; refreshSearchOverlays(); return
  }
  searchBusy.value = true
  const regex = buildSearchRegex()
  if (!regex) { searchBusy.value = false; return }

  const matches: SearchMatch[] = []
  for (let i = 0; i < pageCount.value; i++) {
    regex.lastIndex = 0
    if (renderedPages.value.has(i)) {
      const pageMatches = findAllPageMatchRects(i, regex)
      pageMatches.forEach((rects, j) => matches.push({ pageIndex: i, rects, matchOnPage: j }))
    } else {
      const text = await fetchPageText(i)
      regex.lastIndex = 0
      let m: RegExpExecArray | null
      let j = 0
      while ((m = regex.exec(text)) !== null) {
        matches.push({ pageIndex: i, rects: [], matchOnPage: j })
        j++
        if (m[0].length === 0) regex.lastIndex++
      }
    }
  }
  searchMatches.value = matches
  searchMatchIndex.value = 0
  searchBusy.value = false
  await navigateToSearchMatch(0)
}

async function navigateToSearchMatch(idx: number) {
  if (searchMatches.value.length === 0) { refreshSearchOverlays(); return }
  const n = searchMatches.value.length
  const i = ((idx % n) + n) % n
  searchMatchIndex.value = i

  const match = searchMatches.value[i]
  if (match.rects.length === 0) {
    await ensurePageRendered(match.pageIndex)
    const regex = buildSearchRegex()
    if (regex) {
      const pageMatches = findAllPageMatchRects(match.pageIndex, regex)
      searchMatches.value
        .filter(m => m.pageIndex === match.pageIndex)
        .forEach(m => { if (pageMatches[m.matchOnPage]) m.rects = pageMatches[m.matchOnPage] })
    }
  }
  scrollToPageIndex(match.pageIndex, match.rects[0]?.y ?? 0)
  refreshSearchOverlays()
}

function openSearch() {
  searchOpen.value = true
  nextTick(() => { searchInputRef.value?.select(); searchInputRef.value?.focus() })
}

function closeSearch() {
  searchOpen.value = false
  searchQuery.value = ''
  searchMatches.value = []
  searchMatchIndex.value = 0
  refreshSearchOverlays()
}

// When a page newly renders, populate its match rects and refresh
watch(renderedPages, async () => {
  if (!searchOpen.value || !searchQuery.value.trim()) return
  const regex = buildSearchRegex()
  if (!regex) return
  let changed = false
  const pagesNeedingRects = new Set(
    searchMatches.value
      .filter(m => renderedPages.value.has(m.pageIndex) && m.rects.length === 0)
      .map(m => m.pageIndex)
  )
  for (const pageIndex of pagesNeedingRects) {
    regex.lastIndex = 0
    const pageMatches = findAllPageMatchRects(pageIndex, regex)
    searchMatches.value
      .filter(m => m.pageIndex === pageIndex)
      .forEach(m => { if (pageMatches[m.matchOnPage]) { m.rects = pageMatches[m.matchOnPage]; changed = true } })
  }
  if (changed) refreshSearchOverlays()
})

// Re-apply overlays on scale change (page re-renders handled by unrender→render cycle)
watch(scale, () => { if (searchOpen.value) nextTick(refreshSearchOverlays) })

// Live search as user types (debounced)
let searchDebounce: ReturnType<typeof setTimeout> | null = null
watch(searchQuery, () => {
  if (searchDebounce) clearTimeout(searchDebounce)
  searchDebounce = setTimeout(runSearch, 250)
})
watch([searchCaseSensitive, searchWholeWord, searchHighlightAll], () => {
  if (searchOpen.value) runSearch()
})

const searchCountText = computed(() => {
  const n = searchMatches.value.length
  if (!searchQuery.value.trim()) return ''
  if (n === 0) return searchBusy.value ? '…' : '无结果'
  return `${searchMatchIndex.value + 1} / ${n}`
})

// ── Keyboard navigation ───────────────────────────────────────────────────────
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el?.tagName) return false
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable
}

function onKeyDown(e: KeyboardEvent) {
  const mod = e.metaKey || e.ctrlKey
  // ⌘[ / ⌘] walk the in-document jump history — but not while typing, where the
  // same chord means something else (outdent, bracket entry).
  if (mod && (e.key === '[' || e.key === ']') && !isTypingTarget(e.target)) {
    e.preventDefault()
    if (e.key === '[') jumpHistoryBack()
    else jumpHistoryForward()
    return
  }
  if (mod && e.key === 'f') { e.preventDefault(); openSearch(); return }
  if (mod && e.key === 'g' && searchOpen.value) {
    e.preventDefault()
    navigateToSearchMatch(searchMatchIndex.value + (e.shiftKey ? -1 : 1))
    return
  }
  if (e.key === 'Escape') {
    if (searchOpen.value) { closeSearch(); return }
    hlNotePopup.value = null; hlColorPopup.value = null; selectionPopup.value = null
  }
}

function onWheel(e: WheelEvent) {
  if (!e.metaKey && !e.ctrlKey) return
  e.preventDefault()

  const container = containerRef.value
  if (!container) {
    if (e.deltaY < 0) zoomIn(); else zoomOut()
    return
  }

  const rect = container.getBoundingClientRect()
  const mouseRelX = e.clientX - rect.left
  const mouseRelY = e.clientY - rect.top
  const oldScrollLeft = container.scrollLeft
  const oldScrollTop  = container.scrollTop
  const oldScale = scale.value

  if (e.deltaY < 0) zoomIn(); else zoomOut()

  const ratio = scale.value / oldScale
  if (ratio === 1) return

  nextTick(() => {
    container.scrollLeft = (oldScrollLeft + mouseRelX) * ratio - mouseRelX
    container.scrollTop  = (oldScrollTop  + mouseRelY) * ratio - mouseRelY
  })
}

// ── Trackpad pinch zoom (macOS WKWebView) ─────────────────────────────────────
// Safari/WKWebView reports trackpad pinches as proprietary gesture events
// (gesturestart/gesturechange with a cumulative `scale`), NOT as ctrl+wheel
// like Chromium — so Windows pinch already lands in onWheel above, and this
// handles the Mac. The scale is quantized to 0.125 steps because every scale
// commit tears down and re-renders the visible pages.
let gestureStartScale = 1

function onGestureStart(e: Event) {
  e.preventDefault()
  gestureStartScale = scale.value
}

function onGestureChange(e: Event) {
  e.preventDefault()
  const gs = (e as unknown as { scale?: number }).scale
  if (!gs) return
  const next = Math.max(0.5, Math.min(4, Math.round((gestureStartScale * gs) / 0.125) * 0.125))
  if (next === scale.value) return

  const container = containerRef.value
  const oldScale = scale.value
  if (!container) { scale.value = next; return }

  const rect = container.getBoundingClientRect()
  const ev = e as unknown as { clientX?: number; clientY?: number }
  const relX = (ev.clientX ?? rect.left + rect.width / 2) - rect.left
  const relY = (ev.clientY ?? rect.top + rect.height / 2) - rect.top
  const oldScrollLeft = container.scrollLeft
  const oldScrollTop  = container.scrollTop

  scale.value = next
  const ratio = next / oldScale
  nextTick(() => {
    container.scrollLeft = (oldScrollLeft + relX) * ratio - relX
    container.scrollTop  = (oldScrollTop  + relY) * ratio - relY
  })
}

function onGestureEnd(e: Event) {
  e.preventDefault()
}

// ── Text selection → highlight creation ──────────────────────────────────────
// Collect the selection's rects grouped per page. We DON'T use
// range.getClientRects() directly: on a cross-page selection that also returns
// the box rects of the block elements between the two text runs (page
// containers, canvases, overlays), which are full-page-sized and would highlight
// an entire page. Instead we walk only the text nodes the range touches and take
// rects from a per-text-node sub-range — those are always tight line boxes.
// `skip` holds text-layer spans to leave out (page furniture, see planSelectionFurniture).
function collectSelectionRectsByPage(range: Range, skip?: ReadonlySet<HTMLElement>): { pageIndex: number; rects: Rect[] }[] {
  const rootNode = range.commonAncestorContainer
  const rootEl = (rootNode.nodeType === Node.ELEMENT_NODE ? rootNode : rootNode.parentNode) as HTMLElement | null
  if (!rootEl) return []

  // pageIndex → { pageEl, domRects }
  const byPage = new Map<number, { pageEl: HTMLElement; domRects: DOMRect[] }>()

  const walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (!node.nodeValue) return NodeFilter.FILTER_REJECT
      return range.intersectsNode(node) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT
    },
  })

  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const parent = node.parentElement
    // Only text inside a page's PDF text layer counts.
    if (!parent?.closest('.textLayer')) continue
    if (skip?.has(parent)) continue
    const pageEl = parent.closest('[data-page-index]') as HTMLElement | null
    if (!pageEl) continue
    const pageIndex = Number(pageEl.dataset.pageIndex)

    const startOffset = node === range.startContainer ? range.startOffset : 0
    const endOffset = node === range.endContainer ? range.endOffset : (node.nodeValue?.length ?? 0)
    if (endOffset <= startOffset) continue

    const sub = document.createRange()
    sub.setStart(node, startOffset)
    sub.setEnd(node, endOffset)

    const bucket = byPage.get(pageIndex) ?? { pageEl, domRects: [] }
    for (const r of Array.from(sub.getClientRects())) {
      if (r.width > 0 && r.height > 0) bucket.domRects.push(r)
    }
    byPage.set(pageIndex, bucket)
  }

  const pages: { pageIndex: number; rects: Rect[] }[] = []
  for (const [pageIndex, { pageEl, domRects }] of byPage) {
    const pageRect = pageEl.getBoundingClientRect()
    const rects = domRects.map(r => ({
      x: (r.left - pageRect.left) / scale.value,
      y: (r.top - pageRect.top) / scale.value,
      width: r.width / scale.value,
      height: r.height / scale.value,
    }))
    if (rects.length) pages.push({ pageIndex, rects })
  }
  pages.sort((a, b) => a.pageIndex - b.pageIndex)
  return pages
}

// ── Page furniture in a cross-page selection ────────────────────────────────
// Dragging from the foot of one page into the next also selects whatever lies between the two
// text runs in DOM order: the footnotes and the page number at the foot of the first page, the
// arXiv stamp in its margin, the running head of the next and the figure set at its top. The
// highlight would paint a "2" yellow and store "…composition. 2In contrast…". utils/pageFurniture.ts
// labels those spans from their geometry; this drops the ones that face the page break, and every
// figure / table the selection runs through. It only ever applies to a selection that really
// crosses a page boundary, and it fails SAFE: anything unexpected returns null and the selection is
// left exactly as the browser made it (a skipped footnote cannot be recovered afterwards, a kept
// one can be deleted by hand). There is no "include them" toggle — the user asked for it to go —
// so the skipped spans are shown as not selected (showSkippedSpans): the page itself says what was
// left out, and a selection that starts or ends inside it keeps it.

// The document's "normal" type size, from its first pages. Two selected pages alone get it
// wrong on ~5 % of pages, always towards flagging less, so the document-wide figure is used
// once it has arrived (a few ms of work, started shortly after the PDF loaded).
let docBodyFontSize: number | null = null

async function prefetchBodyFontSize(doc: PDFDocumentProxy) {
  try {
    const pages: FurniturePage[] = []
    for (let i = 1; i <= Math.min(8, doc.numPages); i++) {
      if (pdfDoc.value !== doc) return // closed meanwhile
      const page = await doc.getPage(i)
      const fp = furniturePageFromTextContent((await page.getTextContent()) as unknown as TextContentLike, page.view, page.rotate)
      if (fp) pages.push(fp)
    }
    docBodyFontSize = estimateBodyFontSize(pages)
  } catch {
    // An improvement, not a requirement: without it the selected pages' own mode is used.
  }
}

/** A rendered page's text layer as furniture input, or null while it is not rendered. */
function renderedFurniturePage(pageIndex: number): FurniturePage | null {
  const pageEl = pageRefs.value[pageIndex]
  const layer = pageEl?.querySelector<HTMLElement>('.textLayer')
  return pageEl && layer ? furnitureReading(pageIndex, layer, pageEl).page : null
}

/**
 * A rendered page's spans for the classifier: the painted spans give the identity (which element is
 * which span, where each line ends), pdf.js's text content gives the geometry whenever it describes
 * the same spans. The painted widths are the browser's own (WebKit's differ from Chrome's by up to
 * 5 %), which is enough to close the gutter between two columns and hide a page's footnotes; the
 * content geometry is the same in every engine and is what the classifier was tuned on.
 * `preferContentGeometry` falls back to the painted geometry unless the two list the very same
 * texts, so this can only ever be as good as the plain reading, never worse.
 */
function furnitureReading(pageIndex: number, layer: HTMLElement, pageEl: HTMLElement): SpanReading {
  const reading = spansFromTextLayer(layer, pageEl, scale.value)
  const src = pageTextSources.get(pageIndex)
  const content = src ? furniturePageFromTextContent(src.content, src.view, src.rotate) : null
  return { ...reading, page: preferContentGeometry(reading.page, content) }
}

/** The same text, spaces and line breaks aside. */
function sameText(a: string, b: string): boolean {
  return a.replace(/\s+/g, '') === b.replace(/\s+/g, '')
}

/**
 * A selection that spans pages, read back from the text layers. `text` is the selection's text with a
 * line break at every page boundary — the browser's own has none there: the app shell is
 * `user-select: none` and only the text layers opt back in (App.vue), so WebKit's `toString()` runs
 * the last line of one page into the first of the next ("…composition. 2In contrast"). `furniture` is
 * the trimmed variant when the policy leaves anything out. Null for a single page, and whenever the
 * text layers do not rebuild the browser's text — spaces and line breaks aside, which is all the
 * engines disagree on.
 */
function planSelectionFurniture(
  range: Range,
  all: { pageIndex: number; rects: Rect[] }[],
  plain: string,
): { text: string; furniture: { report: FurnitureDropReport; trimmed: SelectionVariant; skip: Set<HTMLElement> } | null } | null {
  try {
    const idx = all.map(p => p.pageIndex)
    // Consecutive pages only: a gap is a page that is not rendered, which cannot be judged.
    if (idx.length < 2 || idx.some((v, i) => i > 0 && v !== idx[i - 1] + 1)) return null

    const readings: SpanReading[] = []
    for (const i of idx) {
      const pageEl = pageRefs.value[i]
      const layer = pageEl?.querySelector<HTMLElement>('.textLayer')
      if (!pageEl || !layer) return null
      readings.push(furnitureReading(i, layer, pageEl))
    }
    const selected = readings.map(r => selectedTextBySpan(range, r.elements))

    // Self-check: rebuilt WITHOUT dropping anything, the text must be what the browser gave, but
    // for whitespace. If the text layer is not what the module was verified against (spans nested
    // by an overlay, injected text, a layout it misreads) this fails and nothing is trimmed. Line
    // breaks are not compared: WebKit leaves out the one between two pages (see above).
    const asIs = keptSelectionTextAcrossPages(readings.map((r, k) => ({ selected: selected[k], eolAfter: r.eolAfter })))
    if (!sameText(asIs, plain)) {
      const a = asIs.replace(/\s+/g, '')
      const b = plain.replace(/\s+/g, '')
      let at = 0
      while (at < b.length && a[at] === b[at]) at++
      console.debug('page furniture: the text layer does not rebuild the selection; leaving it as is', {
        at, layer: a.slice(Math.max(0, at - 30), at + 30), selection: b.slice(Math.max(0, at - 30), at + 30),
      })
      return null
    }

    // The pages around the selection vouch for repeating running heads (87 % -> 95 % found).
    const neighbours = [idx[0] - 2, idx[0] - 1, idx[idx.length - 1] + 1, idx[idx.length - 1] + 2]
      .filter(i => i >= 0 && i < pageCount.value)
      .map(renderedFurniturePage)
      .filter((p): p is FurniturePage => p !== null)
    const report = planFurnitureDropReport(
      readings.map((r, k) => ({ page: r.page, selected: selected[k] })),
      { bodyFontSize: docBodyFontSize ?? undefined, neighbours },
    )
    if (report.items.length === 0) return { text: asIs, furniture: null } // nothing to skip

    const skip = new Set<HTMLElement>()
    report.drop.forEach((set, k) => set.forEach(i => skip.add(readings[k].elements[i])))
    const pages = collectSelectionRectsByPage(range, skip)
    const text = keptSelectionTextAcrossPages(readings.map((r, k) => ({
      selected: selected[k], eolAfter: r.eolAfter, dropped: report.drop[k],
    }))).trim()
    // Nothing would remain: keep what the user selected.
    if (pages.length === 0 || text === '') return { text: asIs, furniture: null }
    return { text: asIs, furniture: { report, trimmed: { text, pages }, skip } }
  } catch (e) {
    console.warn('page furniture detection failed; keeping the plain selection:', e)
    return null
  }
}

// The text-layer spans the trimmed selection leaves out. While its popup is up they are painted as NOT
// selected (`.sel-skip`, see the styles), so the page shows exactly what a highlight, a translation or a
// copy will take — the browser's own selection still runs through them in DOM order.
let skippedSpansShown: HTMLElement[] = []

function showSkippedSpans(spans: readonly HTMLElement[]) {
  clearSkippedSpans()
  for (const el of spans) {
    if (!el.isConnected) continue
    el.classList.add('sel-skip')
    skippedSpansShown.push(el)
  }
}

function clearSkippedSpans() {
  for (const el of skippedSpansShown) el.classList.remove('sel-skip')
  skippedSpansShown = []
}

// Whatever closes the popup (or replaces it with one that skips nothing) ends the marking.
watch(selectionPopup, p => {
  if (!p?.furniture) clearSkippedSpans()
  if (!p) popupSource = null
})

/** ⌘C copies what the popup holds for a selection across pages: the text with a line break between the
 *  pages, without the spans shown as not selected. */
function onCopySelection(e: ClipboardEvent) {
  const p = selectionPopup.value
  if (!p?.full || !e.clipboardData) return
  // Only the selection the popup was built from (a later one, or one in a note, copies as it is).
  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || !sameText(sel.toString(), p.full.text)) return
  e.clipboardData.setData('text/plain', p.text)
  e.preventDefault()
}

function onWindowMouseUp(e: MouseEvent) {
  // A drag that STARTED in the note popup is a text selection inside it — the
  // release often lands outside, and dismissing there would tear down the very
  // text the user is selecting (and with it the selection they meant to copy).
  if (noteDragOrigin) { noteDragOrigin = false; return }
  // A click inside a popup (a colour, a button) belongs to it. But a drag that began on the page
  // and ends over the selection popup — it sits right under the last selection's end, so a second
  // drag across the same lines lands on it — is a new selection: it must replace the popup, not
  // leave the previous selection's text and actions under the new one.
  const overPopup = (e.target as HTMLElement).closest('.hl-note-popup, .hl-color-popup, .sel-popup')
  if (overPopup && (pressInPopup || !overPopup.classList.contains('sel-popup'))) return
  hlNotePopup.value = null
  // Right-click releases the contextmenu that just opened hlColorPopup — don't dismiss it
  if (e.button !== 2) hlColorPopup.value = null

  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
    selectionPopup.value = null
    return
  }

  // Pre-compute rects NOW while the selection is still active — reading them
  // later (after a mousedown on a color dot) would find it already cleared.
  const range = sel.getRangeAt(0)
  const pages = collectSelectionRectsByPage(range)
  const text = sel.toString().trim()
  if (pages.length === 0) { selectionPopup.value = null; return }

  // Across a page break the text is rebuilt from the text layers (the browser's own runs the two
  // pages together), and the drag also swept up the page number, running head, footnotes, margin
  // stamp and figures / tables that sit between the two text runs: those are left out, while the
  // selection still exists (null = a single page, or a text layer that does not rebuild the
  // selection: the browser's text, unchanged).
  const plan = planSelectionFurniture(range, pages, text)
  const furniture = plan?.furniture ?? null

  // Anchor the toolbar to where the mouse was released rather than the bottom
  // of the selection — feels more direct and stays near the cursor.
  const at = { x: e.clientX, y: e.clientY + 12 }
  selectionPopup.value = furniture && plan
    ? { ...at, text: furniture.trimmed.text, pages: furniture.trimmed.pages, full: { text: plan.text, pages }, furniture: furniture.report }
    : plan
      ? { ...at, text: plan.text, pages, full: { text: plan.text, pages } }
      : { ...at, text, pages }
  if (furniture) showSkippedSpans([...furniture.skip])
  popupSource = { range: range.cloneRange(), skip: furniture?.skip ?? null }
  // A popup that is already open keeps its element (and so its size), so the observer will not
  // fire for the new position: fit it explicitly.
  void nextTick(fitSelectionPopup)
}

function createHighlight(color?: string) {
  const popup = selectionPopup.value
  if (!popup || popup.pages.length === 0) { selectionPopup.value = null; return }

  const c = color ?? activeColor.value
  const created_at = new Date().toISOString()
  // One record per page the selection covers, each with only that page's rects.
  // The records keep the WHOLE selection text and this one shared `created_at` —
  // that pair is what utils/highlightGroups uses to show them as a single highlight,
  // and it is the only marker an older build (which strips fields it does not know)
  // leaves intact. Saved in one batch so no half of a selection is ever on disk.
  reader.addHighlights(popup.pages.map(({ pageIndex, rects }): Highlight => ({
    id: crypto.randomUUID(),
    page: pageIndex + 1,
    rects,
    text: popup.text,
    color: c,
    created_at,
    style: highlightStyle.value,
  })))
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
}

// ── Highlight popup actions ───────────────────────────────────────────────────
async function translateHighlight(hlId: string) {
  const hl = resolveHighlightGroup(hlId)
  if (!hl) return
  hlColorPopup.value = null
  await runTranslation(hl.displayText)
}

// Delete / recolour / note write to every record of the highlight, in one save.
// Touching only the clicked half would leave its twin behind (and the cross-machine
// merge's last-edit-wins would then bring the half-deleted or half-recoloured one back).
function deleteHighlight(id: string) {
  const ids = highlightIdsOf(id)
  reader.removeHighlights(ids)
  ids.forEach(forgetNotePopupSize)
  hlColorPopup.value = null
  hlNotePopup.value = null
}

// A PDF has only printed lines, so a highlight reads as one merged paragraph unless the
// user keeps the original breaks (list, code, equation). Per-highlight, group-wide.
function toggleHighlightLineBreaks(id: string) {
  const g = resolveHighlightGroup(id)
  if (!g) return
  reader.updateHighlights(g.ids, { ...groupAppearance(g), keep_line_breaks: g.keepLineBreaks ? undefined : true })
  hlColorPopup.value = null
}

function changeHighlightColor(id: string, color: string) {
  reader.updateHighlights(highlightIdsOf(id), { color })
  hlColorPopup.value = null
}

function saveNote() {
  if (!hlNotePopup.value) return
  const { hlId } = hlNotePopup.value
  const next = hlNoteText.value || undefined
  // Nothing changed (blur without typing): don't stamp every record as edited.
  const g = resolveHighlightGroup(hlId)
  if ((g?.note || undefined) === next) return
  reader.updateHighlights(g?.ids ?? [hlId], { ...(g ? groupAppearance(g) : {}), note: next })
  // Popup stays open; caller switches back to view mode
}

async function startNoteEdit() {
  hlNoteEditing.value = true
  await nextTick()
  noteTextareaRef.value?.focus()
}

// ── Plan now ──────────────────────────────────────────────────────────────────
// Scroll events and resizes plan on the next frame; these moments (the PDF finished loading, the
// zoom settled) plan straight away. The viewport has moved without a scroll event, so the
// speed estimate starts over.
function triggerInitialRender() {
  scrollTracker.rebase()
  reconcile()
}

// ── Utils ─────────────────────────────────────────────────────────────────────

</script>

<template>
  <div class="pdf-viewer">
    <!-- Toolbar -->
    <div class="pdf-toolbar">
      <div class="toolbar-title" :title="displayOpenTitle">{{ displayOpenTitle }}</div>

      <div class="toolbar-spacer" />

      <button class="related-btn" :title="t('citeGraph.buttonTitle')" @click="openCitationGraph">
        <span class="related-btn-icon" aria-hidden="true">
          <Icon v-if="citationEmojiIcon" :icon="citationEmojiIcon" width="15" height="15" />
          <template v-else>{{ citationEmoji }}</template>
        </span>
        <span class="related-btn-label">{{ t('citeGraph.buttonLabel') }}</span>
      </button>

      <button class="related-btn" :title="t('related.buttonTitle')" @click="openRelatedFromToolbar">
        <span class="related-btn-icon" aria-hidden="true">
          <Icon v-if="relatedEmojiIcon" :icon="relatedEmojiIcon" width="15" height="15" />
          <template v-else>{{ relatedEmoji }}</template>
        </span>
        <span class="related-btn-label">{{ t('related.buttonLabel') }}</span>
        <span v-if="relatedCount" class="related-btn-count">{{ relatedCount }}</span>
      </button>

      <div class="page-indicator" v-if="pageCount > 0">
        <input
          class="page-input"
          type="number"
          :min="1"
          :max="pageCount"
          :value="pageInputValue"
          @change="onPageInputChange"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <span class="page-sep">/ {{ pageCount }}</span>
      </div>

      <div class="zoom-controls">
        <button @click="zoomOut" :title="t('pdf.zoomOut')">−</button>
        <span class="zoom-label">{{ Math.round(scale * 100) }}%</span>
        <button @click="zoomIn" :title="t('pdf.zoomIn')">+</button>
        <button class="fit-btn" @click="fitWidth" :title="t('pdf.fitWidth')">⇔</button>
      </div>

      <div class="color-picker">
        <div
          v-for="c in COLORS"
          :key="c.value"
          class="color-dot"
          :class="{ active: activeColor === c.value }"
          :style="{ background: c.value }"
          :title="c.label"
          @click="activeColor = c.value"
        />
      </div>
    </div>

    <!-- Error -->
    <div v-if="error" class="pdf-error">
      <Icon icon="fluent:error-circle-24-regular" width="32" height="32" />
      <p>{{ error }}</p>
    </div>

    <!-- Loading -->
    <div v-else-if="loading" class="pdf-loading">
      <div class="spinner" />
      <p>{{ t('pdf.loading') }}</p>
    </div>

    <!-- PDF container -->
    <div v-else-if="pageSizes.length > 0" class="pdf-scroll-frame">
      <div
        ref="containerRef"
        class="pdf-container"
        @scroll.passive="onScroll"
        @click="hlNotePopup = null; hlColorPopup = null"
        @wheel="onWheel"
        @gesturestart="onGestureStart"
        @gesturechange="onGestureChange"
        @gestureend="onGestureEnd"
        @pointermove.passive="showScrollThumbs"
      >
        <div class="pdf-pages">
          <div
            v-for="(size, idx) in pageSizes"
            :key="idx"
            :ref="(el) => observePage(el as HTMLDivElement | null, idx)"
            class="page-wrapper"
            :data-page-index="idx"
            :style="{
              width: `${Math.round(size.width * scale)}px`,
              height: `${Math.round(size.height * scale)}px`,
            }"
          />
        </div>
      </div>

      <div
        v-if="scrollThumbs.vertical.visible"
        class="pdf-scroll-thumb pdf-scroll-thumb-y"
        :class="{ visible: scrollThumbsActive }"
        :style="{
          height: `${scrollThumbs.vertical.size}px`,
          transform: `translateY(${scrollThumbs.vertical.offset}px)`,
        }"
        @pointerdown="onThumbPointerDown('v', $event)"
      />
      <div
        v-if="scrollThumbs.horizontal.visible"
        class="pdf-scroll-thumb pdf-scroll-thumb-x"
        :class="{ visible: scrollThumbsActive }"
        :style="{
          width: `${scrollThumbs.horizontal.size}px`,
          transform: `translateX(${scrollThumbs.horizontal.offset}px)`,
        }"
        @pointerdown="onThumbPointerDown('h', $event)"
      />

      <!-- Shortcut hint, shown the first times a link jump happens -->
      <Transition name="jump-hint">
        <div v-if="jumpHintVisible" class="jump-hint">
          <Icon icon="fluent:arrow-hook-up-left-24-regular" width="15" height="15" class="jump-hint-icon" />
          <div class="jump-hint-text">
            <span class="jump-hint-title">{{ t('pdf.jumpHintTitle') }}</span>
            <span class="jump-hint-keys">
              <kbd>{{ jumpModLabel }}[</kbd> {{ t('pdf.jumpHintBack') }}
              <span class="jump-hint-dot">·</span>
              <kbd>{{ jumpModLabel }}]</kbd> {{ t('pdf.jumpHintForward') }}
              <span class="jump-hint-dot">·</span>
              {{ t('pdf.jumpHintMouse') }}
            </span>
          </div>
          <button class="jump-hint-never" @click="dismissJumpHintForever">
            {{ t('pdf.jumpHintNever') }}
          </button>
          <button class="jump-hint-close" :title="t('pdf.jumpHintClose')" @click="hideJumpHint">
            <Icon icon="fluent:dismiss-24-regular" width="13" height="13" />
          </button>
        </div>
      </Transition>
    </div>

    <!-- Selection popup: click a color to immediately highlight -->
    <div
      v-if="selectionPopup"
      ref="selPopupRef"
      class="sel-popup"
      :style="{ left: `${selectionPopup.x}px`, top: `${selectionPopup.y}px` }"
    >
      <div class="sel-colors">
        <div
          v-for="c in COLORS"
          :key="c.value"
          class="sel-color-dot"
          :style="{ background: c.value }"
          :title="c.label"
          @click="createHighlight(c.value)"
        />
      </div>
      <div class="sel-sep" />
      <button
        class="sel-style-btn"
        :class="{ active: highlightStyle === 'underline' }"
        :title="highlightStyle === 'highlight' ? t('pdf.switchUnderline') : t('pdf.switchHighlight')"
        @click="toggleHighlightStyle"
      >
        <Icon v-if="highlightStyle === 'highlight'" icon="fluent:highlight-24-regular" width="16" height="16" />
        <Icon v-else icon="fluent:text-underline-24-regular" width="16" height="16" />
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="addToSnippetLibrary" :title="t('snippets.addToLibrary')">
        <Icon icon="fluent:bookmark-24-regular" width="13" height="13" />
        <span class="sel-translate-label">{{ t('pdf.snippet') }}</span>
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="translateSelection">
        <Icon icon="argus:translate" width="13" height="13" />
        <span class="sel-translate-label">{{ t('pdf.translate') }}</span>
      </button>
      <div class="sel-sep" />
      <button
        class="sel-translate-btn"
        :class="{ active: readingSelection }"
        @click="readAloudSelection('pdf')"
        :title="readingSelection ? t('pdf.readAloudStop') : t('pdf.readAloud')"
      >
        <Icon :icon="readingSelection ? 'fluent:speaker-off-24-regular' : 'fluent:speaker-2-24-regular'" width="13" height="13" />
        <span class="sel-translate-label">{{ readingSelection ? t('pdf.readAloudStop') : t('pdf.readAloud') }}</span>
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="askAiWithSelection" :title="t('pdf.askAi')">
        <Icon icon="fluent:sparkle-24-regular" width="13" height="13" />
        <span class="sel-translate-label">{{ t('pdf.askAi') }}</span>
      </button>
    </div>

    <!-- Highlight note popup: left-click → view; double-click → edit; blur → auto-save -->
    <div
      v-if="hlNotePopup"
      class="hl-note-popup"
      :style="hlNotePopupStyle"
      @click.stop
    >
      <!-- View mode -->
      <div v-if="!hlNoteEditing" class="hl-note-view selectable-text" @dblclick="startNoteEdit">
        <div v-if="hlNoteText" class="hl-note-text" v-html="hlNoteHtml" />
        <span v-else class="hl-note-placeholder">{{ t('pdf.notePlaceholder') }}</span>
      </div>
      <!-- Edit mode -->
      <textarea
        v-else
        ref="noteTextareaRef"
        v-model="hlNoteText"
        class="hl-note-textarea"
        :placeholder="t('pdf.notePlaceholder')"
        @blur="saveNote(); hlNoteEditing = false"
        @keydown.esc.stop="saveNote(); hlNoteEditing = false"
        @keydown.meta.enter.stop="saveNote(); hlNoteEditing = false"
      />
      <div class="hl-note-resizer" :title="t('pdf.noteResize')" @pointerdown="onNoteResizeStart" />
    </div>

    <!-- Highlight context popup: right-click → change color + delete -->
    <div
      v-if="hlColorPopup"
      ref="hlMenuRef"
      class="hl-color-popup"
      :style="{ left: `${hlColorPopup.x}px`, top: `${hlColorPopup.y}px` }"
      @click.stop
    >
      <div class="hl-popup-colors">
        <div
          v-for="c in COLORS"
          :key="c.value"
          class="sel-color-dot"
          :style="{ background: c.value }"
          :title="c.label"
          @click="changeHighlightColor(hlColorPopup!.hlId, c.value)"
        />
      </div>
      <div class="hl-popup-divider" />
      <button class="hl-action-btn" @click="copyHighlightText(hlColorPopup!.hlId)">{{ t('pdf.copy') }}</button>
      <button
        v-if="hlMenuGroup?.hasLineBreaks"
        class="hl-action-btn"
        :title="hlMenuGroup.keepLineBreaks ? t('hl.mergeLinesHint') : t('hl.keepLinesHint')"
        @click="toggleHighlightLineBreaks(hlColorPopup!.hlId)"
      >{{ hlMenuGroup.keepLineBreaks ? t('hl.mergeLines') : t('hl.keepLines') }}</button>
      <button class="hl-action-btn" @click="addHighlightToSnippetLibrary(hlColorPopup!.hlId)">{{ t('pdf.snippet') }}</button>
      <button class="hl-action-btn" @click="translateHighlight(hlColorPopup!.hlId)">{{ t('pdf.translate') }}</button>
      <button class="hl-action-btn danger" @click="deleteHighlight(hlColorPopup!.hlId)">{{ t('pdf.delete') }}</button>
    </div>

    <!-- OCR progress overlay -->
    <div v-if="ocrProgress" class="ocr-status">
      <Icon class="ocr-spin" icon="fluent:spinner-ios-20-filled" width="13" height="13" />
      <span>{{ t('extraction.stageOcrPage', { page: ocrProgress.page, total: ocrProgress.total }) }}</span>
    </div>

    <!-- Search bar (Cmd+F) -->
    <div v-if="searchOpen" class="search-bar" @click.stop>
      <div class="search-input-row">
        <Icon class="search-icon" icon="fluent:search-24-regular" width="14" height="14" />
        <input
          ref="searchInputRef"
          v-model="searchQuery"
          class="search-input"
          placeholder="搜索…"
          @keydown.enter.prevent="navigateToSearchMatch(searchMatchIndex + 1)"
          @keydown.shift.enter.prevent="navigateToSearchMatch(searchMatchIndex - 1)"
          @keydown.esc.stop="closeSearch"
        />
        <span class="search-count" :class="{ 'no-match': searchQuery && !searchBusy && searchMatches.length === 0 }">
          {{ searchCountText }}
        </span>
        <button class="search-nav-btn" :disabled="searchMatches.length === 0" @click="navigateToSearchMatch(searchMatchIndex - 1)" title="上一个 (Shift+Enter)">
          <Icon icon="fluent:chevron-up-24-regular" width="11" height="11" />
        </button>
        <button class="search-nav-btn" :disabled="searchMatches.length === 0" @click="navigateToSearchMatch(searchMatchIndex + 1)" title="下一个 (Enter)">
          <Icon icon="fluent:chevron-down-24-regular" width="11" height="11" />
        </button>
        <div class="search-divider" />
        <button class="search-close-btn" @click="closeSearch" title="关闭 (Esc)">
          <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
        </button>
      </div>
      <div class="search-options-row">
        <label class="search-opt">
          <input type="checkbox" v-model="searchHighlightAll" @change="refreshSearchOverlays" />
          <span>高亮所有</span>
        </label>
        <label class="search-opt">
          <input type="checkbox" v-model="searchCaseSensitive" />
          <span>区分大小写</span>
        </label>
        <label class="search-opt">
          <input type="checkbox" v-model="searchWholeWord" />
          <span>整词</span>
        </label>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pdf-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
  background: var(--bg-secondary);
  position: relative;
}

/* ── Toolbar ── */
.pdf-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: var(--content-header-height);
  padding: 0 10px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  font-size: var(--font-size-sm);
  overflow: hidden;
  position: relative;
}


.toolbar-title {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: left;
  pointer-events: none;
}

.toolbar-spacer { display: none; }

.related-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  transition: background 0.1s;
}
.related-btn:hover { background: var(--bg-tertiary); color: var(--text-primary); }
.related-btn-icon {
  width: 15px;
  height: 15px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  font-size: 12px;
  line-height: 1;
}
.related-btn-count {
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-pill);
  background: var(--accent-light);
  color: var(--accent);
  font-size: 10px;
  font-weight: 600;
}

.page-indicator {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.page-input {
  width: 44px;
  height: 24px;
  text-align: center;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  padding: 0 4px;
}

.page-sep { color: var(--text-tertiary); font-size: var(--font-size-sm); }

.zoom-controls {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.zoom-controls button {
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  font-size: 16px;
  line-height: 1;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 0.1s;
}
.zoom-controls button:hover { background: var(--bg-tertiary); }

.zoom-label {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  min-width: 36px;
  text-align: center;
}

.fit-btn { font-size: 12px !important; }

.color-picker {
  display: flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
  padding-left: 6px;
  border-left: 1px solid var(--border-subtle);
}

.color-dot {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  cursor: pointer;
  border: 2px solid transparent;
  transition: transform 0.1s, border-color 0.1s;
}
.color-dot:hover { transform: scale(1.2); }
.color-dot.active {
  border-color: var(--text-primary);
  transform: scale(1.15);
}

/* ── Error / Loading ── */
.pdf-error, .pdf-loading {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--text-secondary);
}

.pdf-error svg { color: #cc3333; }
.pdf-error p { font-size: var(--font-size-md); color: var(--text-primary); max-width: 300px; text-align: center; }

.spinner {
  width: 28px;
  height: 28px;
  border: 3px solid var(--border-default);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }

/* ── PDF container ── */
.pdf-scroll-frame {
  flex: 1;
  position: relative;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-secondary);
}

/* ── Jump-history hint ─────────────────────────────────────────────────────── */
.jump-hint {
  position: absolute;
  left: 50%;
  bottom: 18px;
  transform: translateX(-50%);
  z-index: 40;
  display: flex;
  align-items: center;
  gap: 10px;
  max-width: min(560px, calc(100% - 32px));
  padding: 9px 10px 9px 14px;
  border-radius: 12px;
  border: 1px solid color-mix(in srgb, var(--text-primary) 12%, transparent);
  /* Frosted glass: a translucent pane over the page, not a solid bar. */
  background: color-mix(in srgb, var(--bg-primary) 72%, transparent);
  backdrop-filter: blur(18px) saturate(180%);
  -webkit-backdrop-filter: blur(18px) saturate(180%);
  box-shadow: var(--shadow-md);
  font-size: 12px;
  color: var(--text-primary);
  user-select: none;
}
.jump-hint-icon {
  flex-shrink: 0;
  color: var(--accent);
}
.jump-hint-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.jump-hint-title {
  font-weight: 600;
  white-space: nowrap;
}
.jump-hint-keys {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  color: var(--text-secondary);
  font-size: 11.5px;
}
.jump-hint-keys kbd {
  font-family: inherit;
  font-size: 11px;
  font-weight: 600;
  padding: 1px 5px;
  border-radius: 4px;
  background: color-mix(in srgb, var(--text-primary) 10%, transparent);
  color: var(--text-primary);
}
.jump-hint-dot { color: var(--text-tertiary); }
.jump-hint-never {
  flex-shrink: 0;
  margin-left: 2px;
  padding: 5px 10px;
  border-radius: 8px;
  font-size: 11.5px;
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--text-primary) 7%, transparent);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.jump-hint-never:hover {
  background: color-mix(in srgb, var(--text-primary) 13%, transparent);
  color: var(--text-primary);
}
.jump-hint-close {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: 6px;
  color: var(--text-tertiary);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.jump-hint-close:hover {
  background: color-mix(in srgb, var(--text-primary) 10%, transparent);
  color: var(--text-primary);
}

.jump-hint-enter-active,
.jump-hint-leave-active {
  transition: opacity 0.18s ease, transform 0.18s ease;
}
.jump-hint-enter-from,
.jump-hint-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(8px);
}

@media (prefers-reduced-motion: reduce) {
  .jump-hint-enter-active,
  .jump-hint-leave-active { transition: none; }
}

.pdf-container {
  width: 100%;
  height: 100%;
  overflow-y: auto;
  overflow-x: auto;
  background: var(--bg-secondary);
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.pdf-container::-webkit-scrollbar {
  width: 0;
  height: 0;
  display: none;
}

.pdf-scroll-thumb {
  position: absolute;
  z-index: 8;
  pointer-events: none;
  opacity: 0;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-tertiary) 58%, transparent);
  transition: opacity 180ms ease, background 120ms ease;
}

.pdf-scroll-thumb.visible {
  opacity: 1;
  pointer-events: auto;
}

.pdf-scroll-thumb:hover {
  background: color-mix(in srgb, var(--text-tertiary) 78%, transparent);
}
.pdf-scroll-thumb:active {
  background: color-mix(in srgb, var(--text-secondary) 90%, transparent);
}

.pdf-scroll-thumb-y {
  top: 0;
  right: 3px;
  width: 8px;
  cursor: grab;
}

.pdf-scroll-thumb-x {
  left: 0;
  bottom: 3px;
  height: 8px;
  cursor: grab;
}

.pdf-scroll-thumb:active {
  cursor: grabbing;
}

.pdf-pages {
  display: flex;
  flex-direction: column;
  align-items: stretch;
  width: 100%;
  min-width: max-content;
  padding: 3px;
  gap: 12px;
  background: var(--bg-secondary);
}

/* ── Page wrapper ── */
.page-wrapper {
  position: relative;
  background: white;
  box-shadow: var(--shadow-md);
  flex-shrink: 0;
  margin-inline: auto;
}

/* ── Canvas (injected dynamically) ── */
:deep(.pdf-canvas) {
  position: absolute;
  top: 0;
  left: 0;
  display: block;
}

/* ── PDF.js text layer — positioned over canvas, pdfjs CSS handles the rest ── */
:deep(.textLayer) {
  position: absolute;
  top: 0;
  left: 0;
}

:deep(.textLayer ::selection) {
  background: rgba(0, 100, 255, 0.25);
}

/* Spans a cross-page selection leaves out (page numbers, running heads, footnotes, margin text,
   figures and tables — planSelectionFurniture): shown as not selected while the popup skips them. */
:deep(.textLayer .sel-skip::selection),
:deep(.textLayer .sel-skip *::selection) {
  background: transparent;
}

/* endOfContent anchor: sits below the layer by default, expands to cover it
   while selecting so drag-into-whitespace stays anchored in reading order. */
:deep(.textLayer .endOfContent) {
  display: block;
  position: absolute;
  inset: 100% 0 0;
  z-index: 0;
  cursor: default;
  user-select: none;
}

:deep(.textLayer.selecting .endOfContent) {
  top: 0;
}

/* ── Highlight overlay ── */
:deep(.highlight-overlay) {
  position: absolute;
  top: 0;
  left: 0;
  pointer-events: none;
  overflow: hidden;
}

/* ── PDF.js annotation layer (links) ── */
/* `inset: 0` (not width/height) is deliberate: pdfjs writes an inline
   `width/height: round(down, var(--total-scale-factor)*Npx, var(--scale-round-x))`.
   When those CSS vars are unset the value is invalid-at-computed-value-time and
   collapses to `auto`, which would shrink an absolutely-positioned layer with only
   top/left to 0×0 — making its percent-positioned links zero-sized and unclickable
   (the text layer below then steals hover, showing an I-beam). `inset: 0` stretches
   it to fill the page box regardless, exactly like pdfjs's own `.textLayer`. */
:deep(.annotationLayer) {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

:deep(.annotationLayer section) {
  position: absolute;
  pointer-events: auto;
}

:deep(.annotationLayer .linkAnnotation > a) {
  display: block;
  width: 100%;
  height: 100%;
  cursor: pointer;
}

/* ── Plain-text URL linkification overlay ── */
:deep(.linkify-overlay) {
  position: absolute;
  top: 0;
  left: 0;
  pointer-events: none;
  overflow: hidden;
}

:deep(.linkify-overlay .text-link) {
  position: absolute;
  pointer-events: auto;
  cursor: pointer;
  background: transparent;
  border-bottom: 1px solid transparent;
}

:deep(.linkify-overlay .text-link:hover) {
  border-bottom-color: var(--accent);
}

:deep(.hl-rect) {
  position: absolute;
  pointer-events: auto;
  cursor: pointer;
  transition: opacity 0.15s;
}

:deep(.hl-rect:hover) { opacity: 0.75; }

:deep(.reading-hl g) {
  opacity: 0.35;
  animation: reading-in 0.18s ease-out;
}

@keyframes reading-in {
  from { opacity: 0; }
}

:deep(.hl-flash) {
  animation: flash 0.8s ease-in-out 2;
}

@keyframes flash {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.2; }
}

/* ── Selection popup ── */
.sel-popup {
  position: fixed;
  z-index: 1000;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 4px 6px;
  display: flex;
  align-items: center;
  gap: 4px;
  /* Natural width whatever `left` is: a fixed box is otherwise sized to the room to its right,
     which would squash the toolbar before fitSelectionPopup() could measure it. Wider than the
     window (very narrow windows, long labels) wraps instead of running off the edge. */
  width: max-content;
  max-width: calc(100vw - 16px);
  flex-wrap: wrap;
}

.sel-colors, .hl-popup-colors {
  display: flex;
  gap: 4px;
  align-items: center;
}

.sel-color-dot {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  cursor: pointer;
  border: 1px solid rgba(0,0,0,0.15);
  transition: transform 0.1s;
}
.sel-color-dot:hover { transform: scale(1.25); }

.sel-sep {
  width: 1px;
  height: 16px;
  background: var(--border-default);
  flex-shrink: 0;
}

.sel-style-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.sel-style-btn:hover { background: var(--bg-hover); color: var(--accent); }
.sel-style-btn.active, .sel-translate-btn.active { color: var(--accent); background: var(--bg-hover); }

.sel-translate-btn {
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 0 6px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.sel-translate-btn:hover { background: var(--bg-hover); color: var(--accent); }
.sel-translate-label {
  font-size: 11px;
  font-weight: 500;
  white-space: nowrap;
}

/* ── Highlight popups ── */
/* The note popup is a resizable window, driven by the .hl-note-resizer grabber
   below rather than CSS `resize` — see utils/notePopup.ts for why. Its size comes
   from the shared inline style and is clamped there, so no max-* here: a CSS cap
   the drag doesn't know about would stop the box while the stored size kept
   growing. Sizing lives on the frame, not the textarea, so view mode resizes too. */
.hl-note-popup {
  position: fixed;
  z-index: 1001;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 8px;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  min-width: 200px;
  min-height: 90px;
}

/* Deliberately bigger than the ~10px native resizer, and stacked above the note
   body so its scroller can't win the hit-test on the corner. */
.hl-note-resizer {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 18px;
  height: 18px;
  z-index: 2;
  cursor: nwse-resize;
  touch-action: none;
  user-select: none;
  -webkit-user-select: none;
}
.hl-note-resizer::after {
  content: '';
  position: absolute;
  right: 4px;
  bottom: 4px;
  width: 7px;
  height: 7px;
  border-right: 2px solid var(--text-tertiary);
  border-bottom: 2px solid var(--text-tertiary);
  opacity: 0.5;
  transition: opacity 0.1s;
}
.hl-note-popup:hover .hl-note-resizer::after { opacity: 0.9; }

.hl-note-view {
  flex: 1;
  min-height: 0;
  overflow: auto;
  cursor: text;
  padding: 2px 0;
}

.hl-note-text {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  line-height: 1.55;
  word-break: break-word;
}

.hl-note-placeholder {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  font-style: italic;
}

.hl-note-textarea {
  flex: 1;
  width: 100%;
  min-height: 0;
  resize: none;
  box-sizing: border-box;
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  padding: 6px 8px;
  line-height: 1.5;
  font-family: inherit;
  outline: none;
}

/* Rendered note body. The popup is narrow, so everything that can overflow gets
   its own scroller and block spacing is tightened — a stray margin reads as a
   layout bug at this size. */
.hl-note-text :deep(> *:first-child) { margin-top: 0; }
.hl-note-text :deep(> *:last-child) { margin-bottom: 0; }
.hl-note-text :deep(p) { margin: 0 0 6px; }
.hl-note-text :deep(ul),
.hl-note-text :deep(ol) { margin: 0 0 6px; padding-left: 18px; }
.hl-note-text :deep(li) { margin: 1px 0; }
.hl-note-text :deep(h1),
.hl-note-text :deep(h2),
.hl-note-text :deep(h3),
.hl-note-text :deep(h4) { font-size: var(--font-size-sm); font-weight: 600; margin: 6px 0 4px; }
.hl-note-text :deep(blockquote) {
  margin: 0 0 6px;
  padding-left: 8px;
  border-left: 2px solid var(--border-default);
  color: var(--text-secondary);
}
.hl-note-text :deep(hr) { margin: 6px 0; border: none; border-top: 1px solid var(--border-subtle); }
.hl-note-text :deep(img) { max-width: 100%; height: auto; }
.hl-note-text :deep(table) { display: block; overflow-x: auto; max-width: 100%; }
.hl-note-text :deep(pre) { max-width: 100%; overflow-x: auto; }
.hl-note-text :deep(.md-code-block) { margin: 6px 0; }

/* KaTeX: display math is centred and scrolls on its own; keep it near body size
   so a formula doesn't tower over the surrounding text in a small popup. */
.hl-note-text :deep(.katex) { font-size: 1.02em; }
.hl-note-text :deep(.katex-display) {
  margin: 6px 0;
  overflow-x: auto;
  overflow-y: hidden;
  /* padding-top too, or the overflow box clips superscripts/roots. */
  padding-top: 0.25em;
  padding-bottom: 2px;
}
.hl-note-text :deep(.katex-display > .katex) { font-size: 1.08em; }

.hl-color-popup {
  position: fixed;
  /* Natural width whatever the click position (a fixed box near the right edge would
     otherwise shrink to the room left of it), and wrap only if even that is wider
     than the window (narrow split, long labels). */
  width: max-content;
  max-width: calc(100vw - 16px);
  flex-wrap: wrap;
  z-index: 1001;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 6px 8px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.hl-popup-divider {
  width: 1px;
  height: 16px;
  background: var(--border-subtle);
  margin: 0 2px;
}

.hl-action-btn {
  font-size: var(--font-size-xs);
  font-weight: 500;
  padding: 3px 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  transition: background 0.1s;
}
.hl-action-btn:hover { background: var(--bg-tertiary); }
.hl-action-btn.danger { color: #cc3333; }
.hl-action-btn.danger:hover { background: #fff0f0; }

/* ── Search overlay ── */
:deep(.search-overlay) {
  position: absolute;
  top: 0; left: 0;
  width: 100%; height: 100%;
  pointer-events: none;
  z-index: 4;
}

/* ── Search bar ── */
.search-bar {
  position: absolute;
  top: 52px;
  right: 18px;
  z-index: 100;
  background: var(--bg-primary, #fff);
  border: 1px solid var(--border-default, #d1d5db);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0,0,0,0.13);
  padding: 8px 10px 7px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 300px;
  user-select: none;
}

.search-input-row {
  display: flex;
  align-items: center;
  gap: 4px;
}

.search-icon {
  color: var(--text-tertiary);
  flex-shrink: 0;
  margin-right: 2px;
}

.search-input {
  flex: 1;
  border: none;
  outline: none;
  background: transparent;
  font-size: 13px;
  color: var(--text-primary);
  min-width: 0;
}

.search-count {
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
  min-width: 44px;
  text-align: right;
}
.search-count.no-match { color: #ef4444; }

.search-nav-btn {
  width: 22px; height: 22px;
  display: flex; align-items: center; justify-content: center;
  border-radius: 5px;
  border: 1px solid var(--border-default, #d1d5db);
  background: var(--bg-secondary, #f9fafb);
  color: var(--text-secondary);
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.1s;
}
.search-nav-btn:hover:not(:disabled) { background: var(--bg-hover, #f3f4f6); color: var(--text-primary); }
.search-nav-btn:disabled { opacity: 0.4; cursor: default; }

.search-divider {
  width: 1px; height: 16px;
  background: var(--border-default, #d1d5db);
  margin: 0 2px; flex-shrink: 0;
}

.search-close-btn {
  width: 22px; height: 22px;
  display: flex; align-items: center; justify-content: center;
  border-radius: 5px;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  flex-shrink: 0;
  border: none;
  transition: background 0.1s, color 0.1s;
}
.search-close-btn:hover { background: var(--bg-hover, #f3f4f6); color: var(--text-primary); }

.search-options-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 2px;
}

.search-opt {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--text-secondary);
  cursor: pointer;
  white-space: nowrap;
}
.search-opt input[type="checkbox"] { accent-color: var(--accent, #6366f1); margin: 0; }
.search-opt:hover { color: var(--text-primary); }

/* OCR progress */
.ocr-status {
  position: absolute;
  bottom: 14px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 6px 14px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: 20px;
  font-size: 12px;
  color: var(--text-secondary);
  box-shadow: 0 2px 8px rgba(0,0,0,0.12);
  pointer-events: none;
  z-index: 30;
}
.ocr-spin {
  animation: ocr-rotate 0.9s linear infinite;
  flex-shrink: 0;
  color: var(--accent);
}
@keyframes ocr-rotate { to { transform: rotate(360deg); } }
</style>
