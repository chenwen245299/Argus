<script setup lang="ts">
// Ebook reader: continuous-scroll rendering of the normalized chapter-HTML
// representation produced by the Rust ebook module. Deliberately mirrors
// PdfViewer's integration patterns (tab lifecycle, reading state, highlight
// popups, selection actions) WITHOUT sharing code — the PDF path stays
// untouched. Highlights anchor to character offsets in each chapter's
// sanitized DOM text, so they survive font-size changes and reflow.
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import DOMPurify from 'dompurify'
import { runTranslation, triggerAskAi } from '../stores/translationHistory'
import { openAddSnippetModal } from '../stores/snippetLibrary'
import { useReaderStore } from '../stores/reader'
import { useLibraryStore } from '../stores/library'
import { titleInitialCaps } from '../utils/text'
import type { EbookManifest, Highlight } from '../types'

// One instance per open ebook tab; `slug` never changes for an instance.
const props = defineProps<{ slug: string }>()

const reader = useReaderStore()
const library = useLibraryStore()
const { t } = useI18n()

const isActiveTab = computed(() => reader.activeSlug === props.slug)

// ── State ─────────────────────────────────────────────────────────────────────
const containerRef = ref<HTMLDivElement | null>(null)
const manifest = ref<EbookManifest | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)

const chapterEls = ref<(HTMLElement | null)[]>([])
const renderedChapters = ref<Set<number>>(new Set())   // 0-based
const renderingChapters = new Set<number>()
const displayChapter = ref(1)                          // 1-based SPINE index of the topmost visible chapter

let observer: IntersectionObserver | null = null
let progressDebounce: ReturnType<typeof setTimeout> | null = null
let resizeDebounce: ReturnType<typeof setTimeout> | null = null

// Blob URLs for book images, keyed by resource href; revoked on unmount.
const blobUrls = new Map<string, string>()
const blobFetches = new Map<string, Promise<string | null>>()

const displayTitle = computed(() => {
  const tab = reader.tabs.find(tb => tb.slug === props.slug)
  return titleInitialCaps(tab?.title ?? manifest.value?.meta.title ?? props.slug)
})

// ── Font size ─────────────────────────────────────────────────────────────────
const FONT_MIN = 14
const FONT_MAX = 28
const FONT_DEFAULT_KEY = 'argus:ebook-font:default'
function fontKey(slug: string) { return `argus:ebook-font:${slug}` }
function loadFontSize(): number {
  const raw = localStorage.getItem(fontKey(props.slug)) ?? localStorage.getItem(FONT_DEFAULT_KEY)
  const n = raw ? parseInt(raw, 10) : NaN
  return isNaN(n) ? 17 : Math.max(FONT_MIN, Math.min(FONT_MAX, n))
}
const fontSize = ref(loadFontSize())

async function setFontSize(next: number) {
  const clamped = Math.max(FONT_MIN, Math.min(FONT_MAX, next))
  if (clamped === fontSize.value) return
  // Keep the reader anchored to the same chapter position across the reflow.
  const anchor = currentPosition()
  fontSize.value = clamped
  try {
    localStorage.setItem(fontKey(props.slug), String(clamped))
    localStorage.setItem(FONT_DEFAULT_KEY, String(clamped))
  } catch {}
  await nextTick()
  if (anchor) scrollToChapter(anchor.chapter - 1, anchor.ratio)
  refreshAllOverlays()
  paintSelection()
}
function fontSmaller() { setFontSize(fontSize.value - 1) }
function fontLarger()  { setFontSize(fontSize.value + 1) }

// Cmd+wheel (mac) / Ctrl+wheel (win) — Windows trackpad pinches also arrive
// as ctrl+wheel in Chromium. Font size moves in 1px steps, so wheel deltas
// are accumulated instead of stepping on every event.
let wheelAccum = 0
const WHEEL_STEP = 40

function onWheelZoom(e: WheelEvent) {
  if (!e.metaKey && !e.ctrlKey) return
  e.preventDefault()
  wheelAccum += e.deltaY
  while (wheelAccum <= -WHEEL_STEP) { wheelAccum += WHEEL_STEP; fontLarger() }
  while (wheelAccum >= WHEEL_STEP)  { wheelAccum -= WHEEL_STEP; fontSmaller() }
}

// macOS trackpad pinch: WKWebView reports it as proprietary gesture events
// (with a cumulative `scale`), not as ctrl+wheel like Chromium.
let gestureStartFont = 17

function onGestureStart(e: Event) {
  e.preventDefault()
  gestureStartFont = fontSize.value
}

function onGestureChange(e: Event) {
  e.preventDefault()
  const gs = (e as unknown as { scale?: number }).scale
  if (!gs) return
  setFontSize(Math.round(gestureStartFont * gs))
}

function onGestureEnd(e: Event) {
  e.preventDefault()
}

// ── Highlight colors / style (same palette + storage key as the PDF viewer) ──
const activeColor = ref('#FFEB3B')
const HIGHLIGHT_STYLE_KEY = 'argus:highlight-style'
const highlightStyle = ref<'highlight' | 'underline'>(
  (localStorage.getItem(HIGHLIGHT_STYLE_KEY) as 'highlight' | 'underline' | null) ?? 'highlight'
)
function toggleHighlightStyle() {
  highlightStyle.value = highlightStyle.value === 'highlight' ? 'underline' : 'highlight'
  localStorage.setItem(HIGHLIGHT_STYLE_KEY, highlightStyle.value)
}
const COLORS = computed(() => [
  { label: t('pdf.yellow'), value: '#FFEB3B' },
  { label: t('pdf.green'),  value: '#A5D6A7' },
  { label: t('pdf.blue'),   value: '#90CAF9' },
  { label: t('pdf.pink'),   value: '#F48FB1' },
  { label: t('pdf.orange'), value: '#FFCC80' },
  { label: t('pdf.purple'), value: '#CE93D8' },
])

// ── Popups ────────────────────────────────────────────────────────────────────
const selectionPopup = ref<{
  x: number
  y: number
  text: string
  chapterIndex: number       // 0-based
  startOffset: number
  endOffset: number
} | null>(null)
const hlNotePopup = ref<{ x: number; y: number; hlId: string } | null>(null)
const hlNoteText = ref('')
const hlNoteEditing = ref(false)
const noteTextareaRef = ref<HTMLTextAreaElement | null>(null)
const hlColorPopup = ref<{ x: number; y: number; hlId: string } | null>(null)

// ── Lifecycle (global listeners follow the ACTIVE tab, not mount/unmount) ────
function addGlobalListeners() {
  window.addEventListener('mouseup', onWindowMouseUp)
  window.addEventListener('keydown', onKeyDown)
  window.addEventListener('resize', onWindowResize)
  document.addEventListener('selectionchange', onSelectionChange)
}
function removeGlobalListeners() {
  window.removeEventListener('mouseup', onWindowMouseUp)
  window.removeEventListener('keydown', onKeyDown)
  window.removeEventListener('resize', onWindowResize)
  document.removeEventListener('selectionchange', onSelectionChange)
  if (selOverlayRef.value) selOverlayRef.value.textContent = ''
  useNativeSel.value = false
}

onMounted(async () => {
  await loadBook()
})

let _savedScrollTop: number | null = null
watch(isActiveTab, (active) => {
  if (active) {
    addGlobalListeners()
    if (_savedScrollTop !== null) {
      const top = _savedScrollTop
      nextTick(() => { if (containerRef.value) containerRef.value.scrollTop = top })
    }
  } else {
    if (containerRef.value) _savedScrollTop = containerRef.value.scrollTop
    removeGlobalListeners()
    flushReadingState()
  }
}, { immediate: true })

onUnmounted(() => {
  removeGlobalListeners()
  observer?.disconnect()
  if (progressDebounce) clearTimeout(progressDebounce)
  if (resizeDebounce) clearTimeout(resizeDebounce)
  for (const url of blobUrls.values()) URL.revokeObjectURL(url)
  blobUrls.clear()
  reader.discardTabState(props.slug)
})

// ── Load ──────────────────────────────────────────────────────────────────────
async function loadBook() {
  loading.value = true
  error.value = null
  const slug = props.slug

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

  let mf: EbookManifest
  try {
    mf = await invoke<EbookManifest>('read_ebook_manifest', { slug })
  } catch (e) {
    const msg = String(e)
    if (msg.includes('No ebook file found')) {
      reader.closeTab(slug)
      return
    }
    error.value = t('ebook.parseError', { msg })
    loading.value = false
    return
  }
  manifest.value = mf
  chapterEls.value = new Array(mf.chapters.length).fill(null)
  loading.value = false
  await nextTick()

  setupObserver()
  await restorePosition()

  // A chapter jump requested before this viewer finished mounting (sections
  // tab / snippet) is missed by the reactive watch — apply it now.
  if (reader.pendingPageJump != null && isActiveTab.value) {
    const target = reader.pendingPageJump
    reader.pendingPageJump = null
    await scrollToChapter(target - 1, 0)
  }

  // unread → reading, same as the PDF viewer
  const entry = library.papers.find(p => p.slug === slug)
  if (entry?.reading_status === 'unread') {
    try {
      await invoke('set_reading_status', { slug, status: 'reading' })
      entry.reading_status = 'reading'
    } catch {}
  }
}

// ── Chapter shells + lazy rendering ───────────────────────────────────────────
/** Rough pre-render height so the scrollbar is meaningful before chapters load. */
function estimatedHeight(chIdx: number): number {
  const ch = manifest.value?.chapters[chIdx]
  if (!ch) return 200
  const width = Math.min(containerRef.value?.clientWidth ?? 800, 46 * fontSize.value)
  const charsPerLine = Math.max(20, width / (fontSize.value * 0.8))
  const lines = Math.ceil(ch.char_count / charsPerLine)
  return Math.max(120, Math.round(lines * fontSize.value * 1.8) + 80)
}

function setupObserver() {
  observer?.disconnect()
  observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        const idx = Number((entry.target as HTMLElement).dataset.chapterIndex)
        if (!isNaN(idx)) renderChapter(idx)
      }
    },
    { root: containerRef.value, rootMargin: '2000px 0px' },
  )
  chapterEls.value.forEach((el) => { if (el) observer!.observe(el) })
}

function observeChapter(el: HTMLElement | null, idx: number) {
  chapterEls.value[idx] = el
  if (el && observer) observer.observe(el)
}

const PURIFY_CONFIG = {
  USE_PROFILES: { html: true },
  FORBID_TAGS: ['style', 'link', 'meta', 'base', 'form', 'input', 'button', 'select', 'textarea', 'iframe', 'object', 'embed', 'video', 'audio', 'canvas', 'dialog'],
  FORBID_ATTR: ['style'],
  ADD_ATTR: ['data-argus-res'],
} as const

async function renderChapter(idx: number) {
  if (renderedChapters.value.has(idx) || renderingChapters.has(idx)) return
  renderingChapters.add(idx)
  try {
    const html = await invoke<string>('read_ebook_chapter', { slug: props.slug, index: idx + 1 })
    const el = chapterEls.value[idx]
    if (!el) return
    const content = el.querySelector('.chapter-content') as HTMLElement | null
    if (!content) return

    const scrollEl = containerRef.value
    const oldHeight = el.offsetHeight
    const wasAbove = scrollEl ? el.offsetTop + oldHeight < scrollEl.scrollTop : false

    content.innerHTML = DOMPurify.sanitize(html, PURIFY_CONFIG as any) as unknown as string
    el.style.minHeight = ''
    renderedChapters.value = new Set(renderedChapters.value).add(idx)

    // A chapter materializing ABOVE the viewport changes the scroll geometry —
    // compensate so the visible text doesn't jump.
    await nextTick()
    if (wasAbove && scrollEl) {
      scrollEl.scrollTop += el.offsetHeight - oldHeight
    }

    resolveChapterImages(content, idx)
    renderChapterHighlights(idx)
  } catch (e) {
    console.error(`Failed to render chapter ${idx + 1}:`, e)
  } finally {
    renderingChapters.delete(idx)
  }
}

async function ensureChapterRendered(idx: number) {
  if (renderedChapters.value.has(idx)) return
  await renderChapter(idx)
  await nextTick()
}

function resolveChapterImages(content: HTMLElement, chapterIdx: number) {
  const imgs = content.querySelectorAll<HTMLImageElement>('img[data-argus-res]')
  imgs.forEach(async (img) => {
    const href = img.getAttribute('data-argus-res')
    if (!href) return
    const url = await fetchResource(href)
    if (url) {
      // Image sizes shift the layout — recompute this chapter's overlays once loaded.
      img.addEventListener('load', () => renderChapterHighlights(chapterIdx), { once: true })
      img.src = url
    } else {
      img.alt = t('ebook.imageMissing')
      img.classList.add('img-missing')
    }
  })
}

async function fetchResource(href: string): Promise<string | null> {
  const cached = blobUrls.get(href)
  if (cached) return cached
  const inflight = blobFetches.get(href)
  if (inflight) return inflight
  const p = (async () => {
    try {
      const res = await invoke<{ mime: string; base64: string }>('read_ebook_resource', {
        slug: props.slug,
        href,
      })
      const bin = atob(res.base64)
      const bytes = new Uint8Array(bin.length)
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
      const url = URL.createObjectURL(new Blob([bytes], { type: res.mime }))
      blobUrls.set(href, url)
      return url
    } catch (e) {
      console.warn(`Ebook resource ${href} failed:`, e)
      return null
    } finally {
      blobFetches.delete(href)
    }
  })()
  blobFetches.set(href, p)
  return p
}

// ── Custom selection painting ─────────────────────────────────────────────────
// WebKit paints native selections over the full LINE BOX (line-height 1.8 →
// tall boxes that swallow the leading, and paragraph gaps get filled too).
// Saved highlights look right because they're drawn from glyph-tight text
// rects — so the live selection is painted the same way: native selection is
// made transparent inside book text and replaced by our own overlay rects.
const selOverlayRef = ref<HTMLDivElement | null>(null)
const useNativeSel = ref(false)          // fallback for absurdly large selections
const MAX_SEL_RECTS = 1200

/** Glyph-tight client rects for a range: sub-range per intersecting text node
 *  (Range.getClientRects alone also returns whole-element line-box rects). */
function collectTightRects(range: Range): DOMRect[] {
  const out: DOMRect[] = []
  if (range.commonAncestorContainer.nodeType === Node.TEXT_NODE) {
    for (const r of Array.from(range.getClientRects())) {
      if (r.width > 0 && r.height > 0) out.push(r)
    }
    return out
  }
  const rootEl = range.commonAncestorContainer as HTMLElement
  const walker = document.createTreeWalker(rootEl, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      if (!node.nodeValue) return NodeFilter.FILTER_REJECT
      return range.intersectsNode(node) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT
    },
  })
  for (let node = walker.nextNode(); node && out.length <= MAX_SEL_RECTS; node = walker.nextNode()) {
    const startOffset = node === range.startContainer ? range.startOffset : 0
    const endOffset = node === range.endContainer ? range.endOffset : (node.nodeValue?.length ?? 0)
    if (endOffset <= startOffset) continue
    const sub = document.createRange()
    sub.setStart(node, startOffset)
    sub.setEnd(node, endOffset)
    for (const r of Array.from(sub.getClientRects())) {
      if (r.width > 0 && r.height > 0) out.push(r)
    }
  }
  return out
}

function onSelectionChange() {
  paintSelection()
}

function paintSelection() {
  const overlay = selOverlayRef.value
  if (!overlay) return
  overlay.textContent = ''
  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
    useNativeSel.value = false
    return
  }
  const range = sel.getRangeAt(0)
  const node = range.commonAncestorContainer
  const anchorEl = (node.nodeType === Node.ELEMENT_NODE ? node : node.parentNode) as HTMLElement | null
  if (!anchorEl || !anchorEl.closest('.ebook-chapters') || !containerRef.value?.contains(anchorEl)) {
    useNativeSel.value = false
    return
  }
  const rects = collectTightRects(range)
  if (rects.length > MAX_SEL_RECTS) {
    // Book-scale selection: painting thousands of divs would jank — let the
    // (uglier but cheap) native selection show instead.
    useNativeSel.value = true
    return
  }
  useNativeSel.value = false
  const base = overlay.getBoundingClientRect()
  const frag = document.createDocumentFragment()
  for (const r of rects) {
    const div = document.createElement('div')
    div.className = 'sel-rect'
    div.style.cssText = `left:${r.left - base.left}px;top:${r.top - base.top}px;width:${r.width}px;height:${r.height}px;`
    frag.appendChild(div)
  }
  overlay.appendChild(frag)
}

// ── Text-offset anchoring ─────────────────────────────────────────────────────
function chapterContentEl(idx: number): HTMLElement | null {
  return chapterEls.value[idx]?.querySelector('.chapter-content') ?? null
}

/** Absolute character offset of (node, offsetInNode) within root's text. */
function textOffsetOf(root: HTMLElement, node: Node, offsetInNode: number): number {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let total = 0
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    if (n === node) return total + offsetInNode
    total += n.nodeValue?.length ?? 0
  }
  // node not a text node (element boundary): count text before it instead
  const w2 = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  total = 0
  for (let n = w2.nextNode(); n; n = w2.nextNode()) {
    const pos = node.compareDocumentPosition(n)
    if (pos & Node.DOCUMENT_POSITION_FOLLOWING) break
    total += n.nodeValue?.length ?? 0
  }
  return total
}

/** Rebuild a DOM Range from character offsets over root's text. */
function rangeFromOffsets(root: HTMLElement, start: number, end: number): Range | null {
  if (end <= start) return null
  const range = document.createRange()
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let total = 0
  let startSet = false
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const len = n.nodeValue?.length ?? 0
    if (!startSet && total + len >= start) {
      range.setStart(n, Math.max(0, start - total))
      startSet = true
    }
    if (startSet && total + len >= end) {
      range.setEnd(n, Math.max(0, end - total))
      return range
    }
    total += len
  }
  if (startSet) {
    // end past the last text node — clamp to document end
    range.setEndAfter(root.lastChild ?? root)
    return range
  }
  return null
}

// ── Selection popup ───────────────────────────────────────────────────────────
function onWindowMouseUp(e: MouseEvent) {
  if ((e.target as HTMLElement).closest('.hl-note-popup, .hl-color-popup, .sel-popup')) return
  hlNotePopup.value = null
  if (e.button !== 2) hlColorPopup.value = null

  const sel = window.getSelection()
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
    selectionPopup.value = null
    return
  }
  const range = sel.getRangeAt(0)

  // The selection must start inside one of OUR chapters.
  const startEl = (range.startContainer.nodeType === Node.ELEMENT_NODE
    ? range.startContainer
    : range.startContainer.parentNode) as HTMLElement | null
  const chapterEl = startEl?.closest('[data-chapter-index]') as HTMLElement | null
  if (!chapterEl || !containerRef.value?.contains(chapterEl)) {
    selectionPopup.value = null
    return
  }
  const chapterIndex = Number(chapterEl.dataset.chapterIndex)
  const content = chapterContentEl(chapterIndex)
  if (!content) { selectionPopup.value = null; return }

  // Clamp cross-chapter selections to the start chapter (documented UX).
  const clamped = range.cloneRange()
  if (!content.contains(range.endContainer)) {
    clamped.setEndAfter(content.lastChild ?? content)
  }

  const startOffset = textOffsetOf(content, clamped.startContainer, clamped.startOffset)
  const endOffset = textOffsetOf(content, clamped.endContainer, clamped.endOffset)
  const text = clamped.toString().trim()
  if (!text || endOffset <= startOffset) { selectionPopup.value = null; return }

  selectionPopup.value = {
    x: e.clientX,
    y: e.clientY + 12,
    text,
    chapterIndex,
    startOffset,
    endOffset,
  }
}

const CONTEXT_CHARS = 32

function createHighlight(color?: string) {
  const popup = selectionPopup.value
  if (!popup) return
  const content = chapterContentEl(popup.chapterIndex)
  const fullText = content?.textContent ?? ''
  const hl: Highlight = {
    id: crypto.randomUUID(),
    page: popup.chapterIndex + 1,
    rects: [],
    text: popup.text,
    color: color ?? activeColor.value,
    created_at: new Date().toISOString(),
    style: highlightStyle.value,
    start_offset: popup.startOffset,
    end_offset: popup.endOffset,
    anchor_prefix: fullText.slice(Math.max(0, popup.startOffset - CONTEXT_CHARS), popup.startOffset),
    anchor_suffix: fullText.slice(popup.endOffset, popup.endOffset + CONTEXT_CHARS),
  }
  reader.addHighlight(hl)
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
}

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

const SNIPPET_HIGHLIGHT_COLOR = '#CE93D8'
function addToSnippetLibrary() {
  const popup = selectionPopup.value
  if (!popup) return
  window.getSelection()?.removeAllRanges()
  selectionPopup.value = null
  const paper = library.papers.find(p => p.slug === props.slug)
  openAddSnippetModal({
    text: popup.text,
    paperId: props.slug,
    paperTitle: paper?.title ?? props.slug,
    page: popup.chapterIndex + 1,
    color: SNIPPET_HIGHLIGHT_COLOR,
  })
}

// ── Highlight overlays ────────────────────────────────────────────────────────
function chapterHighlights(idx: number): Highlight[] {
  return reader.highlightsFor(props.slug).filter(h => h.page === idx + 1)
}

/** Re-anchor a highlight whose stored offsets no longer match its text. */
function reanchor(hl: Highlight, fullText: string): { start: number; end: number } | null {
  const needle = (hl.anchor_prefix ?? '') + hl.text + (hl.anchor_suffix ?? '')
  let at = needle ? fullText.indexOf(needle) : -1
  if (at >= 0) {
    const start = at + (hl.anchor_prefix?.length ?? 0)
    return { start, end: start + hl.text.length }
  }
  at = fullText.indexOf(hl.text)
  if (at >= 0) return { start: at, end: at + hl.text.length }
  return null
}

function renderChapterHighlights(idx: number) {
  const wrapper = chapterEls.value[idx]
  const content = chapterContentEl(idx)
  if (!wrapper || !content || !renderedChapters.value.has(idx)) return
  const overlay = wrapper.querySelector('.chapter-overlay') as HTMLElement | null
  if (!overlay) return
  overlay.innerHTML = ''
  const wrapperRect = wrapper.getBoundingClientRect()
  const fullText = content.textContent ?? ''

  for (const hl of chapterHighlights(idx)) {
    let start = hl.start_offset ?? -1
    let end = hl.end_offset ?? -1
    let range = start >= 0 && end > start ? rangeFromOffsets(content, start, end) : null
    // Verify the anchor still points at the highlighted text; fall back to
    // prefix+text+suffix search when the offsets drifted (sanitizer change …).
    if (!range || normalizeWs(range.toString()) !== normalizeWs(hl.text)) {
      const fixed = reanchor(hl, fullText)
      range = fixed ? rangeFromOffsets(content, fixed.start, fixed.end) : null
      if (fixed && range && isActiveTab.value) {
        reader.updateHighlight(hl.id, { start_offset: fixed.start, end_offset: fixed.end })
      }
    }
    if (!range) continue

    for (const r of collectTightRects(range)) {
      if (r.width <= 0 || r.height <= 0) continue
      const div = document.createElement('div')
      div.dataset.hlId = hl.id
      if ((hl.style ?? 'highlight') === 'underline') {
        div.className = 'hl-rect hl-underline'
        div.style.cssText = `left:${r.left - wrapperRect.left}px;top:${r.top - wrapperRect.top}px;width:${r.width}px;height:${r.height}px;border-bottom:2px solid ${hl.color};`
      } else {
        div.className = 'hl-rect'
        div.style.cssText = `left:${r.left - wrapperRect.left}px;top:${r.top - wrapperRect.top}px;width:${r.width}px;height:${r.height}px;background:${hl.color};`
      }
      div.addEventListener('click', (ev) => {
        ev.stopPropagation()
        hlNotePopup.value = { x: ev.clientX, y: ev.clientY + 8, hlId: hl.id }
        hlNoteText.value = reader.highlightsFor(props.slug).find(h => h.id === hl.id)?.note ?? ''
        hlNoteEditing.value = false
        hlColorPopup.value = null
      })
      div.addEventListener('contextmenu', (ev) => {
        ev.preventDefault()
        ev.stopPropagation()
        hlColorPopup.value = { x: ev.clientX, y: ev.clientY + 8, hlId: hl.id }
        hlNotePopup.value = null
      })
      overlay.appendChild(div)
    }
  }
}

function normalizeWs(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

function refreshAllOverlays() {
  for (const idx of renderedChapters.value) renderChapterHighlights(idx)
}

watch(() => reader.highlightsFor(props.slug), () => { nextTick(refreshAllOverlays) })

function onWindowResize() {
  if (resizeDebounce) clearTimeout(resizeDebounce)
  resizeDebounce = setTimeout(() => {
    refreshAllOverlays()
    paintSelection()
  }, 200)
}

// ── Highlight popup actions ───────────────────────────────────────────────────
async function copyHighlightText(hlId: string) {
  const hl = reader.highlightsFor(props.slug).find(h => h.id === hlId)
  hlColorPopup.value = null
  if (!hl) return
  try { await navigator.clipboard.writeText(hl.text) } catch {}
}
async function translateHighlight(hlId: string) {
  const hl = reader.highlightsFor(props.slug).find(h => h.id === hlId)
  if (!hl) return
  hlColorPopup.value = null
  await runTranslation(hl.text)
}
function addHighlightToSnippetLibrary(hlId: string) {
  const hl = reader.highlightsFor(props.slug).find(h => h.id === hlId)
  if (!hl) return
  hlColorPopup.value = null
  const paper = library.papers.find(p => p.slug === props.slug)
  openAddSnippetModal({
    text: hl.text,
    paperId: props.slug,
    paperTitle: paper?.title ?? props.slug,
    page: hl.page,
    color: hl.color,
  })
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
}
async function startNoteEdit() {
  hlNoteEditing.value = true
  await nextTick()
  noteTextareaRef.value?.focus()
}

// ── Position: save / restore / jump ───────────────────────────────────────────
/** Topmost visible chapter (1-based) + scroll ratio within it. */
function currentPosition(): { chapter: number; ratio: number } | null {
  const scrollEl = containerRef.value
  if (!scrollEl || chapterEls.value.length === 0) return null
  const scrollTop = scrollEl.scrollTop
  for (let i = 0; i < chapterEls.value.length; i++) {
    const el = chapterEls.value[i]
    if (!el) continue
    const bottom = el.offsetTop + el.offsetHeight
    if (bottom > scrollTop + 10 || i === chapterEls.value.length - 1) {
      const ratio = Math.max(0, Math.min(1, (scrollTop - el.offsetTop) / Math.max(1, el.offsetHeight)))
      return { chapter: i + 1, ratio }
    }
  }
  return null
}

function onScroll() {
  updateDisplayChapter()
  if (progressDebounce) clearTimeout(progressDebounce)
  progressDebounce = setTimeout(() => {
    if (isActiveTab.value) flushReadingState()
  }, 700)
}

function updateDisplayChapter() {
  const pos = currentPosition()
  if (pos) displayChapter.value = pos.chapter
}

function flushReadingState() {
  const pos = currentPosition()
  if (!pos) return
  reader.persistReadingState({
    page: pos.chapter,
    scroll_ratio: pos.ratio,
    updated_at: new Date().toISOString(),
  })
}

async function restorePosition() {
  const rs = reader.readingStateFor(props.slug)
  if (!rs || !containerRef.value) {
    // No saved position: render the beginning.
    await ensureChapterRendered(0)
    return
  }
  const idx = Math.max(0, Math.min((manifest.value?.chapters.length ?? 1) - 1, rs.page - 1))
  await scrollToChapter(idx, rs.scroll_ratio)
}

async function scrollToChapter(idx: number, ratio = 0) {
  const scrollEl = containerRef.value
  if (!scrollEl) return
  await ensureChapterRendered(idx)
  const el = chapterEls.value[idx]
  if (!el) return
  scrollEl.scrollTop = el.offsetTop + ratio * el.offsetHeight - (ratio > 0 ? 0 : 8)
  updateDisplayChapter()
}

watch(() => reader.pendingPageJump, async (page) => {
  if (page === null || !isActiveTab.value) return
  reader.pendingPageJump = null
  await scrollToChapter(page - 1, 0)
})

watch(() => reader.scrollToHighlightId, async (id) => {
  if (!id || !isActiveTab.value) return
  const hl = reader.highlightsFor(props.slug).find(h => h.id === id)
  if (!hl) return
  reader.scrollToHighlightId = null
  const idx = hl.page - 1
  await ensureChapterRendered(idx)
  await nextTick()
  const wrapper = chapterEls.value[idx]
  const rectEl = wrapper?.querySelector(`[data-hl-id="${id}"]`) as HTMLElement | null
  if (rectEl && containerRef.value && wrapper) {
    const top = wrapper.offsetTop + parseFloat(rectEl.style.top || '0')
    containerRef.value.scrollTop = Math.max(0, top - 120)
    rectEl.classList.add('hl-flash')
    setTimeout(() => rectEl.classList.remove('hl-flash'), 1000)
  } else {
    await scrollToChapter(idx, 0)
  }
})

// ── Toolbar: chapter indicator / jump ─────────────────────────────────────────
// The spine (our internal "chapter" unit) counts EVERY document in the book —
// cover, copyright, preface … — so spine position 11 can be the book's actual
// chapter 1. When the book has a usable TOC, the toolbar counts and jumps by
// LOGICAL chapters (top-level TOC entries) instead of raw spine position.
const chapterCount = computed(() => manifest.value?.chapters.length ?? 0)
const level1Toc = computed(() => (manifest.value?.toc ?? []).filter(t => t.level === 1))
const useLogicalChapters = computed(() => level1Toc.value.length >= 2)
const displayTotal = computed(() =>
  useLogicalChapters.value ? level1Toc.value.length : chapterCount.value)
const displayCurrent = computed(() => {
  if (!useLogicalChapters.value) return displayChapter.value
  let ord = 1
  for (let i = 0; i < level1Toc.value.length; i++) {
    if (level1Toc.value[i].chapter <= displayChapter.value) ord = i + 1
    else break
  }
  return ord
})

function onChapterInputChange(e: Event) {
  const val = parseInt((e.target as HTMLInputElement).value)
  if (isNaN(val) || val < 1 || val > displayTotal.value) return
  const spineIdx = useLogicalChapters.value ? level1Toc.value[val - 1].chapter - 1 : val - 1
  scrollToChapter(spineIdx, 0)
}

// ── Keyboard ──────────────────────────────────────────────────────────────────
function onKeyDown(e: KeyboardEvent) {
  if (!isActiveTab.value) return
  const target = e.target as HTMLElement
  if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) return
  if (e.metaKey || e.ctrlKey) {
    if (e.key === '=' || e.key === '+') { e.preventDefault(); fontLarger() }
    else if (e.key === '-') { e.preventDefault(); fontSmaller() }
  }
}

function handleBack() {
  if (progressDebounce) { clearTimeout(progressDebounce); progressDebounce = null }
  flushReadingState()
  reader.closePaper()
}

defineExpose({ closeToList: handleBack })
</script>

<template>
  <div class="ebook-viewer">
    <!-- Toolbar -->
    <div class="ebook-toolbar">
      <div class="toolbar-title" :title="displayTitle">{{ displayTitle }}</div>

      <div class="toolbar-spacer" />

      <div class="chapter-indicator" v-if="chapterCount > 0">
        <span class="chapter-word">{{ t('ebook.chapterShort') }}</span>
        <input
          class="chapter-input"
          type="number"
          :min="1"
          :max="displayTotal"
          :value="displayCurrent"
          @change="onChapterInputChange"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <span class="chapter-sep">/ {{ displayTotal }}</span>
      </div>

      <div class="font-controls">
        <button @click="fontSmaller" :title="t('ebook.fontSmaller')" :disabled="fontSize <= FONT_MIN">A−</button>
        <span class="font-label">{{ fontSize }}</span>
        <button @click="fontLarger" :title="t('ebook.fontLarger')" :disabled="fontSize >= FONT_MAX">A+</button>
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
    <div v-if="error" class="ebook-error">
      <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      <p>{{ error }}</p>
    </div>

    <!-- Loading -->
    <div v-else-if="loading" class="ebook-loading">
      <div class="spinner" />
      <p>{{ t('ebook.loading') }}</p>
    </div>

    <!-- Book -->
    <div
      v-else
      ref="containerRef"
      class="ebook-container"
      :class="{ 'native-sel': useNativeSel }"
      :style="{ '--ebook-font-size': `${fontSize}px` }"
      @scroll.passive="onScroll"
      @click="hlNotePopup = null; hlColorPopup = null"
      @wheel="onWheelZoom"
      @gesturestart="onGestureStart"
      @gesturechange="onGestureChange"
      @gestureend="onGestureEnd"
    >
      <div class="ebook-chapters">
        <section
          v-for="(ch, idx) in manifest?.chapters ?? []"
          :key="ch.index"
          :ref="(el) => observeChapter(el as HTMLElement | null, idx)"
          class="chapter-wrapper"
          :data-chapter-index="idx"
          :style="renderedChapters.has(idx) ? undefined : { minHeight: `${estimatedHeight(idx)}px` }"
        >
          <div class="chapter-content" />
          <div class="chapter-overlay" />
        </section>
        <!-- Live-selection rects (glyph-tight, replaces the native selection) -->
        <div ref="selOverlayRef" class="selection-overlay" />
      </div>
    </div>

    <!-- Selection popup -->
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

    <!-- Highlight note popup -->
    <div
      v-if="hlNotePopup"
      class="hl-note-popup"
      :style="{ left: `${hlNotePopup.x}px`, top: `${hlNotePopup.y}px` }"
      @click.stop
    >
      <div v-if="!hlNoteEditing" class="hl-note-view" @dblclick="startNoteEdit">
        <span v-if="hlNoteText" class="hl-note-text">{{ hlNoteText }}</span>
        <span v-else class="hl-note-placeholder">{{ t('pdf.notePlaceholder') }}</span>
      </div>
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

    <!-- Highlight context popup -->
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
      <button class="hl-action-btn" @click="copyHighlightText(hlColorPopup!.hlId)">{{ t('pdf.copy') }}</button>
      <button class="hl-action-btn" @click="addHighlightToSnippetLibrary(hlColorPopup!.hlId)">{{ t('pdf.snippet') }}</button>
      <button class="hl-action-btn" @click="translateHighlight(hlColorPopup!.hlId)">{{ t('pdf.translate') }}</button>
      <button class="hl-action-btn danger" @click="deleteHighlight(hlColorPopup!.hlId)">{{ t('pdf.delete') }}</button>
    </div>
  </div>
</template>

<style scoped>
.ebook-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-width: 0;
  background: var(--bg-primary);
  position: relative;
}

/* ── Toolbar ─────────────────────────────────────────────────────────────── */
.ebook-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--divider);
  background: var(--bg-secondary);
  flex-shrink: 0;
  min-height: 40px;
}
.toolbar-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 40%;
}
.toolbar-spacer { flex: 1; }

.chapter-indicator {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-secondary);
}
.chapter-word { font-size: 11px; }
.chapter-input {
  width: 44px;
  padding: 2px 4px;
  font-size: 12px;
  text-align: center;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
}
.chapter-input::-webkit-outer-spin-button,
.chapter-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }

.font-controls {
  display: flex;
  align-items: center;
  gap: 4px;
}
.font-controls button {
  padding: 2px 8px;
  font-size: 12px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  cursor: pointer;
}
.font-controls button:disabled { opacity: 0.4; cursor: default; }
.font-controls button:not(:disabled):hover { background: var(--bg-hover, var(--bg-secondary)); }
.font-label {
  font-size: 12px;
  color: var(--text-secondary);
  min-width: 20px;
  text-align: center;
}

.color-picker { display: flex; gap: 5px; }
.color-dot {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  cursor: pointer;
  border: 2px solid transparent;
  transition: transform 0.1s;
}
.color-dot:hover { transform: scale(1.15); }
.color-dot.active { border-color: var(--text-primary); }

/* ── States ──────────────────────────────────────────────────────────────── */
.ebook-error, .ebook-loading {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--text-secondary);
  font-size: 13px;
  padding: 24px;
  text-align: center;
}
.spinner {
  width: 28px;
  height: 28px;
  border: 3px solid var(--border-subtle);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: ebook-spin 0.9s linear infinite;
}
@keyframes ebook-spin { to { transform: rotate(360deg); } }

/* ── Book body ───────────────────────────────────────────────────────────── */
.ebook-container {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  position: relative;
}
.ebook-chapters {
  position: relative;
  max-width: min(46em, 92%);
  margin: 0 auto;
  padding: 24px 0 60vh;
  font-size: var(--ebook-font-size, 17px);
}

/* Live-selection overlay: same glyph-tight rect visuals as saved highlights.
   Native selection is hidden inside book text (it paints full line boxes,
   which looks bloated at line-height 1.8) except in the huge-selection
   fallback, where painting thousands of rects would jank. */
.selection-overlay {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 1;
}
.selection-overlay :deep(.sel-rect) {
  position: absolute;
  background: rgba(77, 139, 255, 0.38);
  border-radius: 1px;
}
.ebook-container:not(.native-sel) :deep(.chapter-content::selection),
.ebook-container:not(.native-sel) :deep(.chapter-content *::selection) {
  background: transparent;
}
.ebook-container.native-sel :deep(.chapter-content *::selection) {
  background: rgba(77, 139, 255, 0.38);
}
.chapter-wrapper {
  position: relative;
  padding: 8px 0;
}
.chapter-overlay {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 1;
}

/* Book typography: publisher CSS is stripped upstream; we own the look. */
.chapter-content {
  color: var(--text-primary);
  line-height: 1.8;
  word-break: break-word;
  /* The app shell disables selection globally (App.vue); book text must be
     selectable for highlights / translate / ask-AI, like the PDF text layer. */
  user-select: text;
  -webkit-user-select: text;
  cursor: text;
}
.chapter-content :deep(p) {
  margin: 0 0 0.9em;
  text-align: justify;
}
.chapter-content :deep(h1),
.chapter-content :deep(h2),
.chapter-content :deep(h3),
.chapter-content :deep(h4) {
  color: var(--text-primary);
  line-height: 1.4;
  margin: 1.4em 0 0.7em;
}
.chapter-content :deep(h1) { font-size: 1.5em; }
.chapter-content :deep(h2) { font-size: 1.3em; }
.chapter-content :deep(h3) { font-size: 1.15em; }
.chapter-content :deep(h4) { font-size: 1.05em; }
.chapter-content :deep(img) {
  max-width: 100%;
  height: auto;
  display: block;
  margin: 1em auto;
}
.chapter-content :deep(img.img-missing) {
  min-height: 40px;
  border: 1px dashed var(--border-subtle);
  color: var(--text-secondary);
}
.chapter-content :deep(blockquote) {
  margin: 1em 0;
  padding: 0.2em 1em;
  border-left: 3px solid var(--border-subtle);
  color: var(--text-secondary);
}
.chapter-content :deep(a) { color: var(--accent); text-decoration: none; cursor: default; }
.chapter-content :deep(table) { border-collapse: collapse; margin: 1em 0; max-width: 100%; }
.chapter-content :deep(td), .chapter-content :deep(th) {
  border: 1px solid var(--border-subtle);
  padding: 4px 8px;
}
.chapter-content :deep(pre), .chapter-content :deep(code) {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 0.9em;
  background: var(--bg-secondary);
  border-radius: var(--radius-sm);
}
.chapter-content :deep(pre) { padding: 10px; overflow-x: auto; }
.chapter-content :deep(hr) {
  border: none;
  border-top: 1px solid var(--divider);
  margin: 1.5em auto;
  width: 40%;
}

/* ── Highlight rects ─────────────────────────────────────────────────────── */
.chapter-overlay :deep(.hl-rect) {
  position: absolute;
  pointer-events: auto;
  cursor: pointer;
  opacity: 0.35;
  border-radius: 1px;
  transition: opacity 0.12s;
}
.chapter-overlay :deep(.hl-rect:hover) { opacity: 0.6; }
.chapter-overlay :deep(.hl-rect.hl-underline) {
  background: transparent !important;
  opacity: 0.9;
}
.chapter-overlay :deep(.hl-rect.hl-flash) {
  opacity: 0.75;
}

/* ── Popups (visual parity with PdfViewer) ───────────────────────────────── */
.sel-popup {
  position: fixed;
  z-index: 60;
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
  padding: 6px 8px;
  transform: translateX(-50%);
}
.sel-colors { display: flex; gap: 5px; }
.sel-color-dot {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  cursor: pointer;
  transition: transform 0.1s;
}
.sel-color-dot:hover { transform: scale(1.2); }
.sel-sep { width: 1px; height: 16px; background: var(--divider); }
.sel-style-btn, .sel-translate-btn {
  display: flex;
  align-items: center;
  gap: 4px;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 4px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.sel-style-btn:hover, .sel-translate-btn:hover {
  background: var(--bg-secondary);
  color: var(--text-primary);
}
.sel-style-btn.active { color: var(--accent); }
.sel-translate-label { white-space: nowrap; }

.hl-note-popup {
  position: fixed;
  z-index: 60;
  width: 240px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
  padding: 8px;
}
.hl-note-view { min-height: 40px; font-size: 12.5px; cursor: text; }
.hl-note-text { color: var(--text-primary); white-space: pre-wrap; }
.hl-note-placeholder { color: var(--text-secondary); }
.hl-note-textarea {
  width: 100%;
  resize: vertical;
  font-size: 12.5px;
  padding: 6px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  box-sizing: border-box;
}

.hl-color-popup {
  position: fixed;
  z-index: 60;
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
  padding: 6px 8px;
}
.hl-popup-colors { display: flex; gap: 5px; }
.hl-popup-divider { width: 1px; height: 16px; background: var(--divider); }
.hl-action-btn {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  white-space: nowrap;
}
.hl-action-btn:hover { background: var(--bg-secondary); color: var(--text-primary); }
.hl-action-btn.danger:hover { color: #e5534b; }
</style>
