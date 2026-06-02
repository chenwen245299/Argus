<script setup lang="ts">
import { ref, shallowRef, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
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
})

onUnmounted(() => {
  window.removeEventListener('mouseup', onWindowMouseUp)
  window.removeEventListener('keydown', onKeyDown)
  observer?.disconnect()
  if (progressDebounce) clearTimeout(progressDebounce)
  pdfDoc.value?.destroy()
})

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
    hl.rects.forEach(rect => {
      if (!isFinite(rect.x) || !isFinite(rect.y)) return
      const div = document.createElement('div')
      div.className = 'hl-rect'
      div.style.left   = `${rect.x * s}px`
      div.style.top    = `${rect.y * s}px`
      div.style.width  = `${rect.width * s}px`
      div.style.height = `${rect.height * s}px`
      div.style.background = hexToRgba(hl.color, 0.35)
      div.style.borderBottom = hl.style === 'underline'
        ? `2px solid ${hl.color}`
        : 'none'
      div.dataset.hlId = hl.id
      div.addEventListener('click', (e) => {
        e.stopPropagation()
        const bounding = div.getBoundingClientRect()
        hlNoteText.value = reader.highlights.find(h => h.id === hl.id)?.note ?? ''
        hlNoteEditing.value = false   // always start in view mode
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
  if (progressDebounce) clearTimeout(progressDebounce)
  progressDebounce = setTimeout(flushReadingState, 700)
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
  const containerW = containerRef.value.clientWidth - 32 // 16px padding each side
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

// ── Keyboard navigation ───────────────────────────────────────────────────────
function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') { hlNotePopup.value = null; hlColorPopup.value = null; selectionPopup.value = null }
}

function onWheel(e: WheelEvent) {
  if (!e.metaKey && !e.ctrlKey) return
  e.preventDefault()
  if (e.deltaY < 0) zoomIn()
  else zoomOut()
}

// ── Text selection → highlight creation ──────────────────────────────────────
function onWindowMouseUp(e: MouseEvent) {
  // Dismiss popups on outside click
  if ((e.target as HTMLElement).closest('.hl-note-popup, .hl-color-popup, .sel-popup')) return
  hlNotePopup.value = null
  hlColorPopup.value = null

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
    style: 'highlight',
  }

  reader.addHighlight(hl)
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
}

// ── Highlight popup actions ───────────────────────────────────────────────────
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
function hexToRgba(hex: string, alpha: number): string {
  const h = hex.replace('#', '')
  const r = parseInt(h.substring(0, 2), 16)
  const g = parseInt(h.substring(2, 4), 16)
  const b = parseInt(h.substring(4, 6), 16)
  return `rgba(${r},${g},${b},${alpha})`
}
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
    <div
      v-else-if="pageSizes.length > 0"
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
      <button class="hl-action-btn danger" @click="deleteHighlight(hlColorPopup!.hlId)">{{ t('pdf.delete') }}</button>
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
.pdf-container {
  flex: 1;
  overflow-y: auto;
  overflow-x: auto;
}

.pdf-pages {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 16px;
  gap: 12px;
}

/* ── Page wrapper ── */
.page-wrapper {
  position: relative;
  background: white;
  box-shadow: var(--shadow-md);
  flex-shrink: 0;
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
  mix-blend-mode: multiply;
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
  padding: 6px 8px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.sel-colors, .hl-popup-colors {
  display: flex;
  gap: 5px;
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
</style>
