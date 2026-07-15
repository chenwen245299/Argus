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

// ── Related papers (manual links) ───────────────────────────────────────────
const relatedCount = computed(() =>
  library.papers.find(p => p.slug === props.slug)?.related_ids?.length ?? 0)
function openRelatedFromToolbar(e: MouseEvent) {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
  library.openRelatedPopover(props.slug, { x: r.right, y: r.bottom + 4 })
}

// ── State ─────────────────────────────────────────────────────────────────────
const containerRef = ref<HTMLDivElement | null>(null)
const manifest = ref<EbookManifest | null>(null)
const loading = ref(true)
const error = ref<string | null>(null)

const chapterEls = ref<(HTMLElement | null)[]>([])
const renderedChapters = ref<Set<number>>(new Set())   // 0-based
const renderingChapters = new Set<number>()
const displayChapter = ref(1)                          // 1-based SPINE index of the topmost visible chapter
const displayPage = ref(1)
const pageCount = ref(1)
const pageMarkers = ref<Array<{ page: number; top: number }>>([])

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
  schedulePageMetricsUpdate()
  refreshAllOverlays()
  refreshSearchOverlays()
  paintSelection()
}
function fontSmaller() { setFontSize(fontSize.value - 1) }
function fontLarger()  { setFontSize(fontSize.value + 1) }

function onFontSizeInputChange(e: Event) {
  const input = e.target as HTMLInputElement
  const next = parseInt(input.value, 10)
  if (isNaN(next)) {
    input.value = String(fontSize.value)
    return
  }
  const clamped = Math.max(FONT_MIN, Math.min(FONT_MAX, next))
  input.value = String(clamped)
  void setFontSize(clamped)
}

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
  ADD_ATTR: ['data-argus-res', 'data-argus-link-url', 'data-argus-link-chapter', 'data-argus-link-anchor'],
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
    if (searchOpen.value) renderChapterSearchMatches(idx)
    schedulePageMetricsUpdate()
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
      img.addEventListener('load', () => {
        renderChapterHighlights(chapterIdx)
        schedulePageMetricsUpdate()
      }, { once: true })
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

function cssEscape(value: string): string {
  const escape = (globalThis.CSS as { escape?: (s: string) => string } | undefined)?.escape
  return escape ? escape(value) : value.replace(/[^a-zA-Z0-9_-]/g, ch => `\\${ch}`)
}

function attrSelectorValue(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
}

function anchorCandidates(anchor: string): string[] {
  const candidates = [anchor]
  try {
    const decoded = decodeURIComponent(anchor)
    if (decoded && decoded !== anchor) candidates.push(decoded)
  } catch {}
  return candidates
}

async function openEbookExternalLink(url: string) {
  try {
    await invoke('open_url', { url })
  } catch (e) {
    console.warn('Failed to open ebook link:', e)
  }
}

async function scrollToChapterAnchor(chapter: number, anchor?: string) {
  const max = manifest.value?.chapters.length ?? 0
  if (max <= 0) return
  const idx = Math.max(0, Math.min(max - 1, chapter - 1))
  if (!containerRef.value || idx < 0) return

  await ensureChapterRendered(idx)
  await nextTick()

  const scrollEl = containerRef.value
  const wrapper = chapterEls.value[idx]
  if (!scrollEl || !wrapper) return

  if (anchor) {
    for (const candidate of anchorCandidates(anchor)) {
      const selector = `#${cssEscape(candidate)}, [name="${attrSelectorValue(candidate)}"]`
      const target = wrapper.querySelector<HTMLElement>(selector)
      if (target) {
        const wrapperRect = wrapper.getBoundingClientRect()
        const targetRect = target.getBoundingClientRect()
        scrollEl.scrollTop = Math.max(0, wrapper.offsetTop + targetRect.top - wrapperRect.top - 12)
        updateDisplayChapter()
        updatePageMetrics()
        return
      }
    }
  }

  await scrollToChapter(idx, 0)
}

async function onContainerClick(e: MouseEvent) {
  hlNotePopup.value = null
  hlColorPopup.value = null

  const target = e.target as Element | null
  const link = target?.closest('a[data-argus-link-url], a[data-argus-link-chapter]') as HTMLElement | null
  if (!link || !containerRef.value?.contains(link)) return

  e.preventDefault()
  e.stopPropagation()

  const url = link.dataset.argusLinkUrl
  if (url) {
    await openEbookExternalLink(url)
    return
  }

  const chapter = Number(link.dataset.argusLinkChapter)
  if (!Number.isFinite(chapter) || chapter < 1) return
  await scrollToChapterAnchor(chapter, link.dataset.argusLinkAnchor)
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
    updatePageMetrics()
    refreshAllOverlays()
    refreshSearchOverlays()
    paintSelection()
  }, 200)
}

// ── In-book search (Cmd/Ctrl+F) ─────────────────────────────────────────────
// Matches the PDF reader's search UX. Default scope is the ~10 pages around the
// current position ("nearby"); the user can toggle to the whole book. Matches
// are located by character offset into each chapter's sanitized text — the same
// offset convention used for user highlights — so they map cleanly to DOM
// ranges (rangeFromOffsets) and glyph-tight rects (collectTightRects).
interface EbookSearchMatch {
  chapterIdx: number // 0-based
  charStart: number
  charEnd: number
}
const MAX_SEARCH_MATCHES = 2000
const searchOpen = ref(false)
const searchQuery = ref('')
const searchScope = ref<'nearby' | 'whole'>('nearby')
const searchMatches = ref<EbookSearchMatch[]>([])
const searchMatchIndex = ref(0)
const searchBusy = ref(false)
const searchInputRef = ref<HTMLInputElement | null>(null)
// Plain-text per-chapter cache for chapters not currently in the DOM (whole-book
// search). Sanitized identically to render, so offsets line up on navigation.
const chapterTextCache = new Map<number, string>()
let searchDebounce: ReturnType<typeof setTimeout> | null = null
let searchSeq = 0 // cancels an in-flight search when the query/scope changes

const searchCountLabel = computed(() => {
  if (searchBusy.value) return t('ebook.searching')
  if (!searchQuery.value.trim()) return ''
  const n = searchMatches.value.length
  if (n === 0) return t('ebook.searchNoResults')
  return `${searchMatchIndex.value + 1}/${n}`
})

/** Chapters that intersect a scroll window of ±5 virtual pages (~10 pages). */
function nearbyChapterRange(): [number, number] {
  const total = manifest.value?.chapters.length ?? 0
  if (total === 0) return [0, 0]
  const scrollEl = containerRef.value
  if (!scrollEl) { const c = Math.max(0, displayChapter.value - 1); return [c, c] }
  const margin = 5 * virtualPageStep()
  const top = scrollEl.scrollTop - margin
  const bottom = scrollEl.scrollTop + scrollEl.clientHeight + margin
  let lo = -1
  let hi = -1
  for (let i = 0; i < total; i++) {
    const el = chapterEls.value[i]
    if (!el) continue
    const elTop = el.offsetTop
    const elBottom = elTop + el.offsetHeight
    if (elBottom >= top && elTop <= bottom) {
      if (lo === -1) lo = i
      hi = i
    }
  }
  if (lo === -1) { const c = Math.max(0, displayChapter.value - 1); return [c, c] }
  return [lo, hi]
}

/** Plain text of a chapter, using the live DOM when rendered (exact offsets)
 *  and a sanitized fetch (cached) otherwise. */
async function getChapterText(idx: number): Promise<string> {
  const content = chapterContentEl(idx)
  if (content && renderedChapters.value.has(idx)) return content.textContent ?? ''
  const cached = chapterTextCache.get(idx)
  if (cached != null) return cached
  try {
    const html = await invoke<string>('read_ebook_chapter', { slug: props.slug, index: idx + 1 })
    const div = document.createElement('div')
    div.innerHTML = DOMPurify.sanitize(html, PURIFY_CONFIG as any) as unknown as string
    const text = div.textContent ?? ''
    chapterTextCache.set(idx, text)
    return text
  } catch {
    return ''
  }
}

async function runSearch() {
  const q = searchQuery.value
  const seq = ++searchSeq
  clearSearchOverlays()
  if (!q.trim()) {
    searchMatches.value = []
    searchMatchIndex.value = 0
    searchBusy.value = false
    return
  }
  searchBusy.value = true
  const total = manifest.value?.chapters.length ?? 0
  let lo = 0
  let hi = total - 1
  if (searchScope.value === 'nearby') [lo, hi] = nearbyChapterRange()
  const needle = q.toLowerCase()
  const found: EbookSearchMatch[] = []
  for (let idx = lo; idx <= hi; idx++) {
    const hay = (await getChapterText(idx)).toLowerCase()
    if (seq !== searchSeq) return // superseded by a newer query
    let from = 0
    while (found.length < MAX_SEARCH_MATCHES) {
      const at = hay.indexOf(needle, from)
      if (at < 0) break
      found.push({ chapterIdx: idx, charStart: at, charEnd: at + q.length })
      from = at + Math.max(1, q.length)
    }
    if (found.length >= MAX_SEARCH_MATCHES) break
  }
  if (seq !== searchSeq) return
  searchMatches.value = found
  searchMatchIndex.value = 0
  searchBusy.value = false
  if (found.length) await navigateToMatch(0)
  else refreshSearchOverlays()
}

async function navigateToMatch(i: number) {
  const n = searchMatches.value.length
  if (n === 0) { refreshSearchOverlays(); return }
  const idx = ((i % n) + n) % n
  searchMatchIndex.value = idx
  const m = searchMatches.value[idx]
  await ensureChapterRendered(m.chapterIdx)
  await nextTick()
  const content = chapterContentEl(m.chapterIdx)
  const wrapper = chapterEls.value[m.chapterIdx]
  const scrollEl = containerRef.value
  if (content && wrapper && scrollEl) {
    const range = rangeFromOffsets(content, m.charStart, m.charEnd)
    const rects = range ? collectTightRects(range) : []
    if (rects.length) {
      const wrapperRect = wrapper.getBoundingClientRect()
      const targetTop = wrapper.offsetTop + (rects[0].top - wrapperRect.top)
      scrollEl.scrollTop = Math.max(0, targetTop - scrollEl.clientHeight * 0.3)
    }
  }
  await nextTick()
  refreshSearchOverlays()
}

function nextMatch() { void navigateToMatch(searchMatchIndex.value + 1) }
function prevMatch() { void navigateToMatch(searchMatchIndex.value - 1) }

// Enter / Shift+Enter inside the search box move between matches. (Esc and
// Cmd/Ctrl+G are handled by the global onKeyDown before its input-focus guard.)
function onSearchInputKey(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    if (e.shiftKey) prevMatch()
    else nextMatch()
  }
}

function renderChapterSearchMatches(idx: number) {
  const wrapper = chapterEls.value[idx]
  const content = chapterContentEl(idx)
  if (!wrapper || !content || !renderedChapters.value.has(idx)) return
  const overlay = wrapper.querySelector('.chapter-search-overlay') as HTMLElement | null
  if (!overlay) return
  overlay.innerHTML = ''
  if (!searchOpen.value || !searchQuery.value.trim() || searchMatches.value.length === 0) return
  const wrapperRect = wrapper.getBoundingClientRect()
  const activeIdx = searchMatchIndex.value
  searchMatches.value.forEach((m, mi) => {
    if (m.chapterIdx !== idx) return
    const range = rangeFromOffsets(content, m.charStart, m.charEnd)
    if (!range) return
    const isActive = mi === activeIdx
    for (const r of collectTightRects(range)) {
      if (r.width <= 0 || r.height <= 0) continue
      const div = document.createElement('div')
      div.className = isActive ? 'search-rect active' : 'search-rect'
      div.style.cssText = `left:${r.left - wrapperRect.left}px;top:${r.top - wrapperRect.top}px;width:${r.width}px;height:${r.height}px;`
      overlay.appendChild(div)
    }
  })
}

function refreshSearchOverlays() {
  for (const idx of renderedChapters.value) renderChapterSearchMatches(idx)
}

function clearSearchOverlays() {
  for (const idx of renderedChapters.value) {
    const ov = chapterEls.value[idx]?.querySelector('.chapter-search-overlay') as HTMLElement | null
    if (ov) ov.innerHTML = ''
  }
}

function openSearch() {
  searchOpen.value = true
  nextTick(() => { searchInputRef.value?.focus(); searchInputRef.value?.select() })
  if (searchQuery.value.trim()) void runSearch()
}

function closeSearch() {
  searchOpen.value = false
  searchSeq++ // cancel any in-flight whole-book search
  searchBusy.value = false
  searchMatches.value = []
  searchMatchIndex.value = 0
  clearSearchOverlays()
}

function setSearchScope(scope: 'nearby' | 'whole') {
  if (searchScope.value === scope) return
  searchScope.value = scope
}

watch(searchQuery, () => {
  if (searchDebounce) clearTimeout(searchDebounce)
  if (!searchOpen.value) return
  searchDebounce = setTimeout(() => { void runSearch() }, 250)
})
watch(searchScope, () => { if (searchOpen.value && searchQuery.value.trim()) void runSearch() })

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
  updateDisplayPage()
  if (progressDebounce) clearTimeout(progressDebounce)
  progressDebounce = setTimeout(() => {
    if (isActiveTab.value) flushReadingState()
  }, 700)
}

function updateDisplayChapter() {
  const pos = currentPosition()
  if (pos) displayChapter.value = pos.chapter
}

function virtualPageStep(): number {
  const scrollEl = containerRef.value
  if (!scrollEl) return 1
  const typographicPage = fontSize.value * 58
  const viewportPage = scrollEl.clientHeight * 1.35
  return Math.max(680, Math.floor(Math.max(typographicPage, viewportPage)))
}

function updatePageMetrics() {
  const scrollEl = containerRef.value
  if (!scrollEl) {
    displayPage.value = 1
    pageCount.value = 1
    pageMarkers.value = []
    return
  }
  const step = virtualPageStep()
  const maxScroll = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight)
  const total = Math.max(1, Math.floor(maxScroll / step) + 1)
  pageCount.value = total
  pageMarkers.value = Array.from({ length: Math.max(0, total - 1) }, (_, idx) => ({
    page: idx + 2,
    top: (idx + 1) * step,
  }))
  updateDisplayPage()
}

function updateDisplayPage() {
  const scrollEl = containerRef.value
  if (!scrollEl) return
  const step = virtualPageStep()
  displayPage.value = Math.max(1, Math.min(pageCount.value, Math.floor(scrollEl.scrollTop / step) + 1))
}

function schedulePageMetricsUpdate() {
  nextTick(() => {
    window.requestAnimationFrame(updatePageMetrics)
  })
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
    schedulePageMetricsUpdate()
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
  updatePageMetrics()
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

const displayChapterHint = computed(() => `${displayCurrent.value}/${displayTotal.value}`)

function onPageInputChange(e: Event) {
  const val = parseInt((e.target as HTMLInputElement).value, 10)
  if (isNaN(val)) return
  const target = Math.max(1, Math.min(pageCount.value, val))
  const scrollEl = containerRef.value
  if (!scrollEl) return
  const maxScroll = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight)
  scrollEl.scrollTop = Math.min(maxScroll, (target - 1) * virtualPageStep())
  updateDisplayChapter()
  updatePageMetrics()
}

// ── Keyboard ──────────────────────────────────────────────────────────────────
function onKeyDown(e: KeyboardEvent) {
  if (!isActiveTab.value) return
  const mod = e.metaKey || e.ctrlKey
  // Search shortcuts must run BEFORE the input-focus guard so they work even
  // while the search box itself is focused.
  if (mod && (e.key === 'f' || e.key === 'F')) { e.preventDefault(); openSearch(); return }
  if (searchOpen.value) {
    if (e.key === 'Escape') { e.preventDefault(); closeSearch(); return }
    if (mod && (e.key === 'g' || e.key === 'G')) {
      e.preventDefault()
      if (e.shiftKey) prevMatch()
      else nextMatch()
      return
    }
  }
  const target = e.target as HTMLElement
  if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable) return
  if (mod) {
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

      <button class="related-btn" :title="t('related.buttonTitle')" @click="openRelatedFromToolbar">
        <span class="related-btn-icon">🔗</span>
        <span class="related-btn-label">{{ t('related.buttonLabel') }}</span>
        <span v-if="relatedCount" class="related-btn-count">{{ relatedCount }}</span>
      </button>

      <div class="reading-position" v-if="chapterCount > 0">
        <span class="position-group">
          <span class="control-label">{{ t('ebook.chapterShort') }}</span>
          <span class="position-value">{{ displayChapterHint }}</span>
        </span>
        <span class="position-group">
          <span class="control-label">{{ t('ebook.pageShort') }}</span>
          <input
            class="toolbar-number-input page-input"
            type="number"
            :min="1"
            :max="pageCount"
            :value="displayPage"
            @change="onPageInputChange"
            @keydown.enter="($event.target as HTMLInputElement).blur()"
          />
          <span class="page-sep">/ {{ pageCount }}</span>
        </span>

        <span class="position-group font-controls">
          <span class="control-label">{{ t('ebook.fontSize') }}</span>
          <button class="font-step-btn" @click="fontSmaller" :title="t('ebook.fontSmaller')" :disabled="fontSize <= FONT_MIN">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
          <input
            class="toolbar-number-input font-size-input"
            type="number"
            :min="FONT_MIN"
            :max="FONT_MAX"
            :value="fontSize"
            @change="onFontSizeInputChange"
            @keydown.enter="($event.target as HTMLInputElement).blur()"
          />
          <button class="font-step-btn" @click="fontLarger" :title="t('ebook.fontLarger')" :disabled="fontSize >= FONT_MAX">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
              <line x1="12" y1="5" x2="12" y2="19" />
              <line x1="5" y1="12" x2="19" y2="12" />
            </svg>
          </button>
        </span>
      </div>

      <button class="ebook-search-btn" @click="openSearch" :title="t('ebook.search')">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>

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
      @click="onContainerClick"
      @wheel="onWheelZoom"
      @gesturestart="onGestureStart"
      @gesturechange="onGestureChange"
      @gestureend="onGestureEnd"
    >
      <div class="ebook-chapters">
        <div v-if="pageMarkers.length > 0" class="page-break-layer" aria-hidden="true">
          <div
            v-for="marker in pageMarkers"
            :key="marker.page"
            class="page-break-marker"
            :style="{ top: `${marker.top}px` }"
          >
            <span class="page-break-label">{{ t('ebook.pageShort') }} {{ marker.page }}</span>
          </div>
        </div>
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
          <div class="chapter-search-overlay" />
        </section>
        <!-- Live-selection rects (glyph-tight, replaces the native selection) -->
        <div ref="selOverlayRef" class="selection-overlay" />
      </div>
    </div>

    <!-- In-book search -->
    <div v-if="searchOpen" class="ebook-search-bar" @click.stop>
      <svg class="ebook-search-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
        <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
      </svg>
      <input
        ref="searchInputRef"
        v-model="searchQuery"
        class="ebook-search-input"
        :placeholder="t('ebook.searchPlaceholder')"
        @keydown="onSearchInputKey"
      />
      <span
        class="ebook-search-count"
        :class="{ 'no-match': searchQuery.trim() && !searchBusy && searchMatches.length === 0 }"
      >{{ searchCountLabel }}</span>
      <div class="ebook-search-scope">
        <button
          :class="{ active: searchScope === 'nearby' }"
          @click="setSearchScope('nearby')"
          :title="t('ebook.searchNearbyTip')"
        >{{ t('ebook.searchNearby') }}</button>
        <button
          :class="{ active: searchScope === 'whole' }"
          @click="setSearchScope('whole')"
          :title="t('ebook.searchWholeBookTip')"
        >{{ t('ebook.searchWholeBook') }}</button>
      </div>
      <button class="ebook-search-nav" :disabled="searchMatches.length === 0" @click="prevMatch" :title="t('ebook.searchPrev')">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="18 15 12 9 6 15" /></svg>
      </button>
      <button class="ebook-search-nav" :disabled="searchMatches.length === 0" @click="nextMatch" :title="t('ebook.searchNext')">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9" /></svg>
      </button>
      <button class="ebook-search-close" @click="closeSearch" :title="t('ebook.searchClose')">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></svg>
      </button>
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
  height: var(--content-header-height);
  min-height: var(--content-header-height);
  padding: 0 12px;
  border-bottom: 1px solid var(--divider);
  background: var(--bg-secondary);
  flex-shrink: 0;
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

.related-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  height: 26px;
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
.related-btn-icon { font-size: 12px; }
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

.reading-position {
  display: flex;
  align-items: center;
  gap: 0;
  height: 28px;
  padding: 0 4px;
  border: 1px solid color-mix(in srgb, var(--border-default) 70%, var(--border-subtle));
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-secondary) 74%, var(--bg-primary));
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--bg-primary) 42%, transparent);
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
}
.position-group {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 100%;
  padding: 0 10px;
  position: relative;
}
.position-group + .position-group::before {
  content: '';
  position: absolute;
  left: 0;
  top: 6px;
  bottom: 6px;
  width: 1px;
  background: color-mix(in srgb, var(--text-tertiary) 32%, var(--divider));
}
.control-label {
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 500;
}
.position-value {
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 500;
}
.toolbar-number-input {
  height: 24px;
  padding: 0 4px;
  font-size: 12px;
  text-align: center;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-primary);
  transition: background 0.12s, border-color 0.12s;
}
.toolbar-number-input:hover,
.toolbar-number-input:focus {
  border-color: var(--border-subtle);
  background: var(--bg-primary);
  outline: none;
}
.page-input {
  width: 48px;
}
.page-input::-webkit-outer-spin-button,
.page-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
.page-sep {
  min-width: 36px;
  color: var(--text-secondary);
}

.font-controls {
  gap: 4px;
  padding-right: 4px;
}
.font-step-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.font-step-btn:disabled {
  opacity: 0.35;
  cursor: default;
}
.font-step-btn:not(:disabled):hover {
  background: var(--bg-hover, var(--bg-secondary));
  color: var(--text-primary);
}
.font-size-input {
  width: 34px;
}
.font-size-input::-webkit-outer-spin-button,
.font-size-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }

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
.page-break-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 2;
}
.page-break-marker {
  position: absolute;
  right: -88px;
  width: 82px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: flex-start;
  transform: translateY(-12px);
  color: color-mix(in srgb, var(--text-secondary) 84%, var(--text-tertiary));
  opacity: 0.9;
}
.page-break-marker::before {
  content: '';
  width: 26px;
  height: 2px;
  margin-right: 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-tertiary) 42%, var(--divider));
}
.page-break-label {
  display: inline-flex;
  align-items: center;
  height: 22px;
  padding: 0 8px;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--bg-primary) 96%, var(--bg-secondary));
  color: inherit;
  font-size: 11px;
  font-weight: 600;
  line-height: 1;
  white-space: nowrap;
  box-shadow:
    0 0 0 1px color-mix(in srgb, var(--text-tertiary) 24%, var(--divider)),
    0 4px 12px rgba(15, 23, 42, 0.08);
  backdrop-filter: blur(6px);
}
@media (max-width: 960px) {
  .page-break-marker {
    right: 6px;
    width: auto;
    opacity: 0.78;
  }
  .page-break-marker::before {
    display: none;
  }
  .page-break-label {
    background: color-mix(in srgb, var(--bg-primary) 78%, transparent);
  }
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
  z-index: 1;
}
.chapter-overlay {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 1;
}
.chapter-search-overlay {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 1;
}
.search-rect {
  position: absolute;
  background: rgba(255, 213, 79, 0.42);
  border-radius: 2px;
}
.search-rect.active {
  background: rgba(255, 150, 0, 0.5);
  box-shadow: 0 0 0 1.5px rgba(240, 120, 0, 0.9);
}

/* ── In-book search ── */
.ebook-search-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
  flex-shrink: 0;
}
.ebook-search-btn:hover {
  background: rgba(0, 0, 0, 0.06);
  color: var(--text-primary);
}
.ebook-search-bar {
  position: absolute;
  top: calc(var(--content-header-height) + 8px);
  right: 16px;
  z-index: 100;
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--bg-primary);
  border: 1px solid var(--divider);
  border-radius: 10px;
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.13);
  padding: 6px 8px;
  user-select: none;
}
.ebook-search-icon {
  color: var(--text-tertiary);
  flex-shrink: 0;
}
.ebook-search-input {
  border: 0;
  outline: 0;
  background: transparent;
  font-size: 13px;
  color: var(--text-primary);
  width: 180px;
  min-width: 120px;
}
.ebook-search-count {
  font-size: 12px;
  color: var(--text-tertiary);
  min-width: 42px;
  text-align: right;
  white-space: nowrap;
}
.ebook-search-count.no-match {
  color: #e5484d;
}
.ebook-search-scope {
  display: inline-flex;
  border: 1px solid var(--divider);
  border-radius: 7px;
  overflow: hidden;
  flex-shrink: 0;
}
.ebook-search-scope button {
  border: 0;
  background: transparent;
  cursor: pointer;
  font-size: 11px;
  padding: 3px 8px;
  color: var(--text-secondary);
  white-space: nowrap;
}
.ebook-search-scope button.active {
  background: #5b8def;
  color: #fff;
}
.ebook-search-nav,
.ebook-search-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: 0;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  flex-shrink: 0;
}
.ebook-search-nav:hover:not(:disabled),
.ebook-search-close:hover {
  background: rgba(0, 0, 0, 0.06);
  color: var(--text-primary);
}
.ebook-search-nav:disabled {
  opacity: 0.4;
  cursor: default;
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
.chapter-content :deep(a) { color: var(--accent); text-decoration: none; cursor: pointer; }
.chapter-content :deep(a:hover) { text-decoration: underline; }
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
