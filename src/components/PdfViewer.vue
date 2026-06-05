<script setup lang="ts">
import { ref, shallowRef, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { startTranslation, appendTranslationChunk, finishTranslation, failTranslation, triggerAskAi } from '../stores/translationHistory'
import { openAddSnippetModal } from '../stores/snippetLibrary'
import * as pdfjsLib from 'pdfjs-dist'
import type { PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist'
import { useReaderStore } from '../stores/reader'
import { useLibraryStore } from '../stores/library'
import { titleInitialCaps } from '../utils/text'
import type { Highlight, Rect } from '../types'

// ── Worker (local bundle, offline-safe) ──────────────────────────────────────
import PDFWorkerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
pdfjsLib.GlobalWorkerOptions.workerSrc = PDFWorkerUrl

// ── Store & i18n ──────────────────────────────────────────────────────────────
const reader = useReaderStore()
const library = useLibraryStore()
const { t } = useI18n()

// ── State ──────────────────────────────────────────────────────────────────────
const containerRef = ref<HTMLDivElement | null>(null)
const pageRefs = ref<(HTMLDivElement | null)[]>([])

const pdfDoc = shallowRef<PDFDocumentProxy | null>(null)
const pageCount = ref(0)
const pageSizes = ref<{ width: number; height: number }[]>([]) // at scale=1
const renderedPages = ref<Set<number>>(new Set())
const renderingPages = new Set<number>() // guard against concurrent renders

const scale = ref(1.25)
const displayPage = ref(1) // shown in toolbar (1-based)
const pageInputValue = ref('1')
const displayOpenTitle = computed(() => titleInitialCaps(reader.openTitle))
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
  const slug = reader.openSlug
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
const selectionPopup = ref<{ x: number; y: number; pageIndex: number; rects: Rect[]; text: string } | null>(null)
const activeColor = ref('#FFEB3B') // default yellow

const HIGHLIGHT_STYLE_KEY = 'argus:highlight-style'
const highlightStyle = ref<'highlight' | 'underline'>(
  (localStorage.getItem(HIGHLIGHT_STYLE_KEY) as 'highlight' | 'underline' | null) ?? 'highlight'
)
function toggleHighlightStyle() {
  highlightStyle.value = highlightStyle.value === 'highlight' ? 'underline' : 'highlight'
  localStorage.setItem(HIGHLIGHT_STYLE_KEY, highlightStyle.value)
}
const hlNotePopup = ref<{ x: number; y: number; hlId: string } | null>(null)   // left-click: note view/edit
const hlNoteText = ref('')
const hlNoteEditing = ref(false)   // false = view mode, true = edit mode
const noteTextareaRef = ref<HTMLTextAreaElement | null>(null)
const hlColorPopup = ref<{ x: number; y: number; hlId: string } | null>(null)  // right-click: color + delete

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

// IntersectionObserver for lazy rendering
let observer: IntersectionObserver | null = null

// ── Computed ──────────────────────────────────────────────────────────────────
const sortedHighlights = computed(() => {
  return [...reader.highlights].sort((a, b) => {
    if (a.page !== b.page) return a.page - b.page
    const aY = a.rects[0]?.y ?? 0
    const bY = b.rects[0]?.y ?? 0
    return aY - bY
  })
})

function pageHighlights(pageIndex: number): Highlight[] {
  // pageIndex is 0-based; Highlight.page is 1-based
  return reader.highlights.filter(h => h.page === pageIndex + 1)
}

// ── Lifecycle ──────────────────────────────────────────────────────────────────
onMounted(async () => {
  await loadPdf()
  window.addEventListener('mouseup', onWindowMouseUp)
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('argus-snippet-highlight', onSnippetHighlight)
  window.addEventListener('resize', updateScrollThumbs)
})

onUnmounted(() => {
  window.removeEventListener('mouseup', onWindowMouseUp)
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('argus-snippet-highlight', onSnippetHighlight)
  window.removeEventListener('resize', updateScrollThumbs)
  observer?.disconnect()
  if (progressDebounce) clearTimeout(progressDebounce)
  if (scrollThumbHideTimer) clearTimeout(scrollThumbHideTimer)
  if (searchDebounce) clearTimeout(searchDebounce)
  pageTextCache.clear()
  _translateUnlisten?.()
  pdfDoc.value?.destroy()
})

// ── Inline translation ────────────────────────────────────────────────────────
let _translateUnlisten: UnlistenFn | null = null

async function startStreamTranslate(text: string) {
  _translateUnlisten?.()
  _translateUnlisten = null

  const eventId = crypto.randomUUID()
  startTranslation(text)

  try {
    _translateUnlisten = await listen<{ delta: string; done: boolean }>(
      `translate-stream-${eventId}`,
      ({ payload }) => {
        if (payload.done) {
          finishTranslation()
          _translateUnlisten?.()
          _translateUnlisten = null
          return
        }
        appendTranslationChunk(payload.delta)
      },
    )
    await invoke('translate_text_stream', { text, eventId })
  } catch (e) {
    failTranslation(String(e))
    _translateUnlisten?.()
    _translateUnlisten = null
  }
}

async function translateSelection() {
  if (!selectionPopup.value) return
  const { text } = selectionPopup.value
  selectionPopup.value = null
  await startStreamTranslate(text)
}

function askAiWithSelection() {
  if (!selectionPopup.value) return
  const { text } = selectionPopup.value
  selectionPopup.value = null
  triggerAskAi(text)
}

const SNIPPET_HIGHLIGHT_COLOR = '#CE93D8'

function addToSnippetLibrary() {
  const popup = selectionPopup.value
  if (!popup || popup.rects.length === 0) { selectionPopup.value = null; return }
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
  const paper = library.papers.find(p => p.slug === reader.activeSlug)
  openAddSnippetModal({
    text: popup.text,
    paperId: reader.activeSlug ?? '',
    paperTitle: paper?.title ?? reader.activeSlug ?? '',
    page: popup.pageIndex + 1,
    color: SNIPPET_HIGHLIGHT_COLOR,
    rects: popup.rects,
    pageIndex: popup.pageIndex,
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
  const hl = reader.highlights.find(h => h.id === hlId)
  if (!hl) return
  hlColorPopup.value = null
  const paper = library.papers.find(p => p.slug === reader.activeSlug)
  openAddSnippetModal({
    text: hl.text,
    paperId: reader.activeSlug ?? '',
    paperTitle: paper?.title ?? reader.activeSlug ?? '',
    page: hl.page,
    color: hl.color,
  })
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

// ── Load PDF ──────────────────────────────────────────────────────────────────
async function loadPdf() {
  loading.value = true
  error.value = null
  const slug = reader.openSlug
  if (!slug) return
  scale.value = loadSavedZoom(slug) ?? 1.0 // temporary; replaced by fitWidth below if no saved zoom

  // Load highlights and reading state
  try {
    const [hls, rs] = await Promise.all([
      invoke<Highlight[]>('get_highlights', { slug }),
      invoke<{ page: number; scroll_ratio: number; updated_at: string } | null>('get_reading_state', { slug }),
    ])
    reader.setHighlights(hls)
    reader.setReadingState(rs)
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
    const loadingTask = pdfjsLib.getDocument({ data: uint8, isOffscreenCanvasSupported: false })
    const doc = await loadingTask.promise
    pdfDoc.value = doc
    reader.setPdfDoc(doc)
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

    // If no saved zoom, default to fit-width instead of a fixed scale
    if (loadSavedZoom(slug) === null) fitWidth()

    setupObserver()
    await restorePosition()
    triggerInitialRender()
    updateScrollThumbs()

    // Auto-extract fulltext via pdfjs if lopdf/pdftotext extraction previously failed
    extractFulltextIfNeeded(doc, slug)

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

// ── IntersectionObserver ──────────────────────────────────────────────────────
function setupObserver() {
  if (!containerRef.value) return
  observer?.disconnect()
  observer = new IntersectionObserver(
    entries => {
      entries.forEach(entry => {
        const idx = Number((entry.target as HTMLElement).dataset.pageIndex)
        if (entry.isIntersecting) {
          renderPage(idx)
        } else if (!entry.isIntersecting && renderedPages.value.has(idx)) {
          // Only evict pages that are far away (rootMargin keeps nearby pages alive)
          unrenderPage(idx)
        }
      })
    },
    {
      root: containerRef.value,
      rootMargin: '600px 0px',
      threshold: 0,
    }
  )

  pageRefs.value.forEach((el, idx) => {
    if (el) observer!.observe(el)
  })
}

function observePage(el: HTMLDivElement | null, idx: number) {
  pageRefs.value[idx] = el
  if (el && observer) observer.observe(el)
}

// ── Render / Unrender pages ────────────────────────────────────────────────────
async function renderPage(idx: number) {
  if (!pdfDoc.value) return
  if (renderedPages.value.has(idx)) return
  if (renderingPages.has(idx)) return
  renderingPages.add(idx)

  const el = pageRefs.value[idx]
  if (!el) { renderingPages.delete(idx); return }

  try {
    const page: PDFPageProxy = await pdfDoc.value.getPage(idx + 1)
    const dpr = window.devicePixelRatio || 1
    // Logical viewport for CSS layout / text layer / highlights
    const logicalVp = page.getViewport({ scale: scale.value })
    // Physical viewport for crisp canvas rendering on HiDPI screens
    const physicalVp = page.getViewport({ scale: scale.value * dpr })

    // Canvas — pdfjs v5 takes the canvas element directly and owns the context
    const canvas = document.createElement('canvas')
    canvas.className = 'pdf-canvas'
    canvas.width = Math.round(physicalVp.width)
    canvas.height = Math.round(physicalVp.height)
    canvas.style.width = `${Math.round(logicalVp.width)}px`
    canvas.style.height = `${Math.round(logicalVp.height)}px`
    el.appendChild(canvas)

    await page.render({ canvas, viewport: physicalVp }).promise

    // Text layer at logical scale so CSS positions match layout
    const textLayerDiv = document.createElement('div')
    textLayerDiv.className = 'textLayer'
    // pdfjs v5 uses --total-scale-factor to size the container via setLayerDimensions
    textLayerDiv.style.setProperty('--total-scale-factor', String(scale.value))
    el.appendChild(textLayerDiv)

    try {
      const textLayer = new pdfjsLib.TextLayer({
        textContentSource: page.streamTextContent(),
        container: textLayerDiv,
        viewport: logicalVp,
      })
      await textLayer.render()
    } catch (e) {
      console.warn('TextLayer render failed:', e)
    }

    // Highlight overlay at logical scale
    const hlDiv = document.createElement('div')
    hlDiv.className = 'highlight-overlay'
    hlDiv.style.width = `${Math.round(logicalVp.width)}px`
    hlDiv.style.height = `${Math.round(logicalVp.height)}px`
    el.appendChild(hlDiv)

    renderHighlightsOnPage(hlDiv, idx)

    renderedPages.value = new Set(renderedPages.value).add(idx)
    page.cleanup()
  } catch (e) {
    console.error(`renderPage(${idx}) failed:`, e)
  } finally {
    renderingPages.delete(idx)
  }
}

function unrenderPage(idx: number) {
  const el = pageRefs.value[idx]
  if (!el) return
  // Keep the placeholder size — only remove rendered children
  while (el.firstChild) el.removeChild(el.firstChild)
  const next = new Set(renderedPages.value)
  next.delete(idx)
  renderedPages.value = next
}

// ── Re-render on scale change ─────────────────────────────────────────────────
watch(scale, async () => {
  saveZoom()
  // Evict all rendered pages; observer will re-render the visible ones
  const toEvict = [...renderedPages.value]
  toEvict.forEach(unrenderPage)
  await nextTick()
  updateScrollThumbs()
  // Force observer to fire by briefly disconnecting & reconnecting
  if (observer && containerRef.value) {
    observer.disconnect()
    pageRefs.value.forEach((el) => { if (el) observer!.observe(el) })
  }
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
          const bounding = div.getBoundingClientRect()
          hlNoteText.value = reader.highlights.find(h => h.id === hl.id)?.note ?? ''
          hlNoteEditing.value = false
          hlNotePopup.value = { x: bounding.left, y: bounding.bottom + 4, hlId: hl.id }
          hlColorPopup.value = null
        })
        div.addEventListener('contextmenu', (e) => {
          e.preventDefault()
          e.stopPropagation()
          hlColorPopup.value = { x: e.clientX, y: e.clientY + 4, hlId: hl.id }
          hlNotePopup.value = null
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
          const bounding = r.getBoundingClientRect()
          hlNoteText.value = reader.highlights.find(h => h.id === hl.id)?.note ?? ''
          hlNoteEditing.value = false
          hlNotePopup.value = { x: bounding.left, y: bounding.bottom + 4, hlId: hl.id }
          hlColorPopup.value = null
        })
        r.addEventListener('contextmenu', (e) => {
          e.preventDefault()
          e.stopPropagation()
          hlColorPopup.value = { x: e.clientX, y: e.clientY + 4, hlId: hl.id }
          hlNotePopup.value = null
        })
        g.appendChild(r)
      })

      g.addEventListener('mouseenter', () => { g.setAttribute('opacity', '0.6') })
      g.addEventListener('mouseleave', () => { g.setAttribute('opacity', '0.35') })

      svg.appendChild(g)
      container.appendChild(svg)
    }
  })
}

// Re-render highlight overlays when highlights change
watch(() => reader.highlights, () => {
  renderedPages.value.forEach(idx => {
    const el = pageRefs.value[idx]
    if (!el) return
    const overlay = el.querySelector('.highlight-overlay') as HTMLDivElement | null
    if (overlay) renderHighlightsOnPage(overlay, idx)
  })
}, { deep: true })

// Re-render highlight overlays when scale changes (handled by full page re-render above)

// ── Jump to highlight ─────────────────────────────────────────────────────────
watch(() => reader.pendingPageJump, async (page) => {
  if (page === null) return
  reader.pendingPageJump = null
  const pageIndex = page - 1
  await ensurePageRendered(pageIndex)
  scrollToPageIndex(pageIndex, 0)
})

watch(() => reader.scrollToHighlightId, async (id) => {
  if (!id) return
  const hl = reader.highlights.find(h => h.id === id)
  if (!hl) return
  reader.scrollToHighlightId = null
  const pageIndex = hl.page - 1
  await ensurePageRendered(pageIndex)
  scrollToPageIndex(pageIndex, hl.rects[0]?.y ?? 0)
  // Flash the highlight
  setTimeout(() => {
    const el = pageRefs.value[pageIndex]
    if (!el) return
    const hlEl = el.querySelector(`[data-hl-id="${id}"]`) as HTMLDivElement | null
    if (hlEl) {
      hlEl.classList.add('hl-flash')
      setTimeout(() => hlEl.classList.remove('hl-flash'), 1000)
    }
  }, 100)
})

async function ensurePageRendered(pageIndex: number) {
  if (renderedPages.value.has(pageIndex)) return
  await renderPage(pageIndex)
  await nextTick()
}

// ── Progress tracking ─────────────────────────────────────────────────────────
function onScroll() {
  updateDisplayPage()
  showScrollThumbs()
  if (progressDebounce) clearTimeout(progressDebounce)
  progressDebounce = setTimeout(flushReadingState, 700)
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
  scrollThumbHideTimer = setTimeout(() => {
    scrollThumbsActive.value = false
  }, SCROLL_THUMB_HIDE_DELAY)
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
  if (!containerRef.value || pageSizes.value.length === 0) return
  const scrollTop = containerRef.value.scrollTop
  const gap = 12
  let cumY = 0
  for (let i = 0; i < pageSizes.value.length; i++) {
    const pageH = pageSizes.value[i].height * scale.value + gap
    if (cumY + pageH > scrollTop + 10 || i === pageSizes.value.length - 1) {
      const ratio = Math.max(0, Math.min(1, (scrollTop - cumY) / pageH))
      reader.persistReadingState({
        page: i + 1,
        scroll_ratio: ratio,
        updated_at: new Date().toISOString(),
      })
      return
    }
    cumY += pageH
  }
}

// ── Restore scroll position ───────────────────────────────────────────────────
async function restorePosition() {
  const rs = reader.readingState
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
  const pageW = pageSizes.value[0].width
  scale.value = Math.max(0.5, Math.min(4, +(containerW / pageW).toFixed(3)))
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

interface SearchMatch { pageIndex: number; rects: Rect[] }

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

function findPageMatchRects(pageIndex: number, regex: RegExp): Rect[] {
  const el = pageRefs.value[pageIndex]
  if (!el) return []
  const textLayer = el.querySelector('.textLayer')
  if (!textLayer) return []

  // Collect leaf spans (pdfjs renders each text item as a direct-child span)
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

  const rects: Rect[] = []
  regex.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = regex.exec(fullText)) !== null) {
    const ms = m.index, me = ms + m[0].length
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
            rects.push({
              x: (cr.left - pageRect.left) / scale.value,
              y: (cr.top  - pageRect.top)  / scale.value,
              width:  cr.width  / scale.value,
              height: cr.height / scale.value,
            })
          }
        }
      } catch { /* ignore */ }
    }
    if (m[0].length === 0) regex.lastIndex++
  }
  return rects
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
      const rects = findPageMatchRects(i, regex)
      if (rects.length > 0) matches.push({ pageIndex: i, rects })
    } else {
      const text = await fetchPageText(i)
      regex.lastIndex = 0
      if (regex.test(text)) matches.push({ pageIndex: i, rects: [] })
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
    if (regex) match.rects = findPageMatchRects(match.pageIndex, regex)
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
  for (const match of searchMatches.value) {
    if (renderedPages.value.has(match.pageIndex) && match.rects.length === 0) {
      regex.lastIndex = 0
      match.rects = findPageMatchRects(match.pageIndex, regex)
      changed = true
    }
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
function onKeyDown(e: KeyboardEvent) {
  const mod = e.metaKey || e.ctrlKey
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

// ── Text selection → highlight creation ──────────────────────────────────────
function onWindowMouseUp(e: MouseEvent) {
  // Dismiss popups on outside click
  if ((e.target as HTMLElement).closest('.hl-note-popup, .hl-color-popup, .sel-popup')) return
  hlNotePopup.value = null
  // Right-click releases the contextmenu that just opened hlColorPopup — don't dismiss it
  if (e.button !== 2) hlColorPopup.value = null

  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
    selectionPopup.value = null
    return
  }

  // Find which page the selection starts on
  const range = sel.getRangeAt(0)
  const anchor = range.startContainer.parentElement
  const pageEl = anchor?.closest('[data-page-index]') as HTMLElement | null
  if (!pageEl) { selectionPopup.value = null; return }

  const pageIndex = Number(pageEl.dataset.pageIndex)
  const domRects = Array.from(range.getClientRects())
  if (domRects.length === 0) { selectionPopup.value = null; return }

  // Pre-compute rects NOW while selection is active.
  // If we read them later in createHighlight(), mousedown on a color dot will have already cleared the selection.
  const pageRect = pageEl.getBoundingClientRect()
  const rects: Rect[] = domRects
    .filter(r => r.width > 0 && r.height > 0)
    .map(r => ({
      x: (r.left - pageRect.left) / scale.value,
      y: (r.top - pageRect.top) / scale.value,
      width: r.width / scale.value,
      height: r.height / scale.value,
    }))
  const text = sel.toString().trim()
  if (rects.length === 0) { selectionPopup.value = null; return }

  const lastRect = domRects[domRects.length - 1]
  selectionPopup.value = {
    x: lastRect.left,
    y: lastRect.bottom + 6,
    pageIndex,
    rects,
    text,
  }
}

function createHighlight(color?: string) {
  const popup = selectionPopup.value
  if (!popup || popup.rects.length === 0) { selectionPopup.value = null; return }

  const hl: Highlight = {
    id: crypto.randomUUID(),
    page: popup.pageIndex + 1,
    rects: popup.rects,
    text: popup.text,
    color: color ?? activeColor.value,
    created_at: new Date().toISOString(),
    style: highlightStyle.value,
  }

  reader.addHighlight(hl)
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
}

// ── Highlight popup actions ───────────────────────────────────────────────────
async function translateHighlight(hlId: string) {
  const hl = reader.highlights.find(h => h.id === hlId)
  if (!hl) return
  hlColorPopup.value = null
  await startStreamTranslate(hl.text)
}

function deleteHighlight(id: string) {
  reader.removeHighlight(id)
  hlColorPopup.value = null
  hlNotePopup.value = null
}

function changeHighlightColor(id: string, color: string) {
  reader.updateHighlight(id, { color })
  hlColorPopup.value = null
}

function saveNote() {
  if (!hlNotePopup.value) return
  reader.updateHighlight(hlNotePopup.value.hlId, { note: hlNoteText.value || undefined })
  // Popup stays open; caller switches back to view mode
}

async function startNoteEdit() {
  hlNoteEditing.value = true
  await nextTick()
  noteTextareaRef.value?.focus()
}

// ── Fallback initial render ───────────────────────────────────────────────────
// IntersectionObserver is async — explicitly kick-start pages near the viewport
// in case the callback hasn't fired yet after setup.
function triggerInitialRender() {
  if (!containerRef.value) return
  const scrollTop = containerRef.value.scrollTop
  const containerH = containerRef.value.clientHeight || 800
  const margin = 800
  const gap = 12
  let cumY = 0
  for (let i = 0; i < pageSizes.value.length; i++) {
    const pageH = pageSizes.value[i].height * scale.value + gap
    if (cumY + pageH > scrollTop - margin && cumY < scrollTop + containerH + margin) {
      renderPage(i)
    }
    if (cumY > scrollTop + containerH + margin) break
    cumY += pageH
  }
}

// ── Utils ─────────────────────────────────────────────────────────────────────

</script>

<template>
  <div class="pdf-viewer">
    <!-- Toolbar -->
    <div class="pdf-toolbar">
      <div class="toolbar-title" :title="displayOpenTitle">{{ displayOpenTitle }}</div>

      <div class="toolbar-spacer" />

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
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
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
      />
      <div
        v-if="scrollThumbs.horizontal.visible"
        class="pdf-scroll-thumb pdf-scroll-thumb-x"
        :class="{ visible: scrollThumbsActive }"
        :style="{
          width: `${scrollThumbs.horizontal.size}px`,
          transform: `translateX(${scrollThumbs.horizontal.offset}px)`,
        }"
      />
    </div>

    <!-- Selection popup: click a color to immediately highlight -->
    <div
      v-if="selectionPopup"
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
        <svg v-if="highlightStyle === 'highlight'" width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          <rect x="1.5" y="1.5" width="13" height="13" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>
          <text x="8" y="12" text-anchor="middle" font-size="10" font-weight="bold" font-family="serif">A</text>
        </svg>
        <svg v-else width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          <text x="8" y="11" text-anchor="middle" font-size="11" font-weight="bold" font-family="serif">A</text>
          <rect x="1" y="13" width="14" height="1.5" rx="0.75"/>
        </svg>
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="addToSnippetLibrary" :title="t('snippets.addToLibrary')">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/>
        </svg>
        <span class="sel-translate-label">{{ t('pdf.snippet') }}</span>
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="translateSelection">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M5 8l6 6"/>
          <path d="M4 14l6-6 2-3"/>
          <path d="M2 5h12"/>
          <path d="M7 2h1"/>
          <path d="M22 22l-5-10-5 10"/>
          <path d="M14 18h6"/>
        </svg>
        <span class="sel-translate-label">{{ t('pdf.translate') }}</span>
      </button>
      <div class="sel-sep" />
      <button class="sel-translate-btn" @click="askAiWithSelection" :title="t('pdf.askAi')">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor">
          <path d="M12 2l2.09 6.26L20 10l-5.91 1.74L12 18l-2.09-6.26L4 10l5.91-1.74L12 2z"/>
          <path d="M19 14l.9 2.6 2.6.9-2.6.9-.9 2.6-.9-2.6-2.6-.9 2.6-.9.9-2.6z"/>
          <path d="M4 18l.6 1.8 1.8.6-1.8.6-.6 1.8-.6-1.8-1.8-.6 1.8-.6.6-1.8z"/>
        </svg>
        <span class="sel-translate-label">{{ t('pdf.askAi') }}</span>
      </button>
    </div>

    <!-- Highlight note popup: left-click → view; double-click → edit; blur → auto-save -->
    <div
      v-if="hlNotePopup"
      class="hl-note-popup"
      :style="{ left: `${hlNotePopup.x}px`, top: `${hlNotePopup.y}px` }"
      @click.stop
    >
      <!-- View mode -->
      <div v-if="!hlNoteEditing" class="hl-note-view" @dblclick="startNoteEdit">
        <span v-if="hlNoteText" class="hl-note-text">{{ hlNoteText }}</span>
        <span v-else class="hl-note-placeholder">{{ t('pdf.notePlaceholder') }}</span>
      </div>
      <!-- Edit mode -->
      <textarea
        v-else
        ref="noteTextareaRef"
        v-model="hlNoteText"
        class="hl-note-textarea"
        rows="3"
        :placeholder="t('pdf.notePlaceholder')"
        @blur="saveNote(); hlNoteEditing = false"
        @keydown.esc.stop="saveNote(); hlNoteEditing = false"
        @keydown.meta.enter.stop="saveNote(); hlNoteEditing = false"
      />
    </div>

    <!-- Highlight context popup: right-click → change color + delete -->
    <div
      v-if="hlColorPopup"
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
      <button class="hl-action-btn" @click="addHighlightToSnippetLibrary(hlColorPopup!.hlId)">{{ t('pdf.snippet') }}</button>
      <button class="hl-action-btn" @click="translateHighlight(hlColorPopup!.hlId)">{{ t('pdf.translate') }}</button>
      <button class="hl-action-btn danger" @click="deleteHighlight(hlColorPopup!.hlId)">{{ t('pdf.delete') }}</button>
    </div>

    <!-- OCR progress overlay -->
    <div v-if="ocrProgress" class="ocr-status">
      <svg class="ocr-spin" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <path d="M21 12a9 9 0 1 1-6.219-8.56"/>
      </svg>
      <span>{{ t('extraction.stageOcrPage', { page: ocrProgress.page, total: ocrProgress.total }) }}</span>
    </div>

    <!-- Search bar (Cmd+F) -->
    <div v-if="searchOpen" class="search-bar" @click.stop>
      <div class="search-input-row">
        <svg class="search-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
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
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="18 15 12 9 6 15"/></svg>
        </button>
        <button class="search-nav-btn" :disabled="searchMatches.length === 0" @click="navigateToSearchMatch(searchMatchIndex + 1)" title="下一个 (Enter)">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <div class="search-divider" />
        <button class="search-close-btn" @click="closeSearch" title="关闭 (Esc)">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
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
  height: 40px;
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
  transition: opacity 180ms ease;
}

.pdf-scroll-thumb.visible {
  opacity: 1;
}

.pdf-scroll-thumb-y {
  top: 0;
  right: 4px;
  width: 5px;
}

.pdf-scroll-thumb-x {
  left: 0;
  bottom: 4px;
  height: 5px;
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

/* ── Highlight overlay ── */
:deep(.highlight-overlay) {
  position: absolute;
  top: 0;
  left: 0;
  pointer-events: none;
  overflow: hidden;
}

:deep(.hl-rect) {
  position: absolute;
  pointer-events: auto;
  cursor: pointer;
  transition: opacity 0.15s;
}

:deep(.hl-rect:hover) { opacity: 0.75; }

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
.sel-style-btn.active { color: var(--accent); background: var(--bg-hover); }

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
.hl-note-popup {
  position: fixed;
  z-index: 1001;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 8px;
  width: 220px;
}

.hl-note-view {
  min-height: 48px;
  cursor: text;
  padding: 2px 0;
}

.hl-note-text {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}

.hl-note-placeholder {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  font-style: italic;
}

.hl-note-textarea {
  width: 100%;
  min-height: 72px;
  resize: vertical;
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

.hl-color-popup {
  position: fixed;
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
