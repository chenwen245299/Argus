import { defineStore } from 'pinia'
import { ref, computed, shallowRef } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Highlight, ReadingState } from '../types'
import type { PDFDocumentProxy } from 'pdfjs-dist'
import { recordPaperAccess } from '../utils/recentPapers'

export interface Tab {
  slug: string
  title: string
  /** Main document format (see PaperMeta.file_type). Absent = pdf.
   *  Tabs persisted before this field existed lack it — MainView falls back
   *  to the library index when routing to a viewer. */
  fileType?: string
}

export const useReaderStore = defineStore('reader', () => {
  const tabs      = ref<Tab[]>([])
  const activeSlug = ref<string | null>(null)

  // Backward-compat computed props (PdfViewer uses these)
  const openSlug  = computed(() => activeSlug.value)
  const openTitle = computed(() => tabs.value.find(t => t.slug === activeSlug.value)?.title ?? '')

  // Per-slug state, keyed by slug and kept across tab switches. This lets each
  // tab's PdfViewer be preserved (via <KeepAlive>) and switched to instantly:
  // its highlights/reading-state are still here, so nothing reloads from disk.
  const pdfDoc              = shallowRef<PDFDocumentProxy | null>(null) // active tab's doc
  const highlightsBySlug    = ref<Record<string, Highlight[]>>({})
  const readingStateBySlug  = ref<Record<string, ReadingState | null>>({})
  // Transient commands aimed at the currently-active viewer.
  const scrollToHighlightId = ref<string | null>(null)
  const pendingPageJump     = ref<number | null>(null)

  const EMPTY_HIGHLIGHTS: Highlight[] = []
  // Active-tab views — the right sidebar / highlights tab read these.
  const highlights = computed<Highlight[]>(() =>
    activeSlug.value ? (highlightsBySlug.value[activeSlug.value] ?? EMPTY_HIGHLIGHTS) : EMPTY_HIGHLIGHTS)
  const readingState = computed<ReadingState | null>(() =>
    activeSlug.value ? (readingStateBySlug.value[activeSlug.value] ?? null) : null)

  // Per-slug accessors — each PdfViewer reads its OWN tab's data (not the active
  // tab's) so a backgrounded viewer keeps showing the right highlights.
  function highlightsFor(slug: string): Highlight[] {
    return highlightsBySlug.value[slug] ?? EMPTY_HIGHLIGHTS
  }
  function readingStateFor(slug: string): ReadingState | null {
    return readingStateBySlug.value[slug] ?? null
  }

  // Free a tab's cached per-slug state. Called when the viewer instance is
  // actually destroyed (evicted from KeepAlive / closed) or its paper is gone.
  function discardTabState(slug: string) {
    if (slug in highlightsBySlug.value) {
      const next = { ...highlightsBySlug.value }; delete next[slug]; highlightsBySlug.value = next
    }
    if (slug in readingStateBySlug.value) {
      const next = { ...readingStateBySlug.value }; delete next[slug]; readingStateBySlug.value = next
    }
  }

  function openPaper(slug: string, title: string, fileType?: string) {
    recordPaperAccess(slug)
    const existing = tabs.value.find(t => t.slug === slug)
    if (!existing) {
      tabs.value.push({ slug, title, fileType })
    } else {
      existing.title = title  // update title in case it changed
      if (fileType && !existing.fileType) existing.fileType = fileType
    }
    activeSlug.value = slug
  }

  function replacePaperSlug(oldSlug: string, newSlug: string, title?: string) {
    const existing = tabs.value.find(t => t.slug === oldSlug)
    if (existing) {
      existing.slug = newSlug
      if (title) existing.title = title
    }
    if (activeSlug.value === oldSlug) {
      activeSlug.value = newSlug
    }
  }

  function switchTab(slug: string) {
    if (activeSlug.value === slug) return
    if (!tabs.value.find(t => t.slug === slug)) return
    // Note: switching to an already-open tab is NOT a new "open" — recency is
    // recorded only in openPaper, so "最近阅读" stays ordered by open order.
    activeSlug.value = slug
  }

  function closeTab(slug: string) {
    const idx = tabs.value.findIndex(t => t.slug === slug)
    if (idx === -1) return
    tabs.value.splice(idx, 1)
    if (activeSlug.value === slug) {
      const next = tabs.value[Math.min(idx, tabs.value.length - 1)]
      activeSlug.value = next?.slug ?? null
    }
    // Note: per-slug state is freed when the viewer instance is actually
    // destroyed (see PdfViewer's discardTabState on unmount), NOT here — the
    // KeepAlive'd instance may linger in cache and be reused if reopened.
  }

  function closePaper() {
    if (activeSlug.value) closeTab(activeSlug.value)
  }

  function showList() {
    activeSlug.value = null
  }

  function reorderTabs(fromIdx: number, insertBefore: number) {
    if (fromIdx === insertBefore || fromIdx + 1 === insertBefore) return
    const arr = [...tabs.value]
    const [item] = arr.splice(fromIdx, 1)
    const adjusted = fromIdx < insertBefore ? insertBefore - 1 : insertBefore
    arr.splice(adjusted, 0, item)
    tabs.value = arr
  }

  /** Remove any tabs whose slugs are no longer in the library paper list. */
  function pruneStaleTabs(validSlugs: Set<string>) {
    const before = tabs.value.length
    const removed = tabs.value.filter(t => !validSlugs.has(t.slug))
    tabs.value = tabs.value.filter(t => validSlugs.has(t.slug))
    removed.forEach(t => discardTabState(t.slug))
    if (tabs.value.length !== before && activeSlug.value && !validSlugs.has(activeSlug.value)) {
      activeSlug.value = tabs.value[0]?.slug ?? null
    }
  }

  function _tabKey(libraryPath: string) {
    return `argus:tabs:${libraryPath}`
  }

  function saveTabs(libraryPath: string) {
    try {
      localStorage.setItem(_tabKey(libraryPath), JSON.stringify({
        tabs: tabs.value,
        activeSlug: activeSlug.value,
      }))
    } catch {}
  }

  function loadTabs(libraryPath: string) {
    try {
      const raw = localStorage.getItem(_tabKey(libraryPath))
      if (!raw) return
      const state = JSON.parse(raw) as { tabs: Tab[]; activeSlug: string | null }
      tabs.value = Array.isArray(state.tabs) ? state.tabs : []
      activeSlug.value = state.activeSlug ?? null
    } catch {}
  }

  function setPdfDoc(doc: PDFDocumentProxy, slug?: string) {
    // Guard against a slow async PDF load for a now-inactive tab clobbering the
    // freshly-switched tab's document. Only apply when the load's slug (if
    // provided) still matches the active tab.
    if (slug !== undefined && slug !== activeSlug.value) return
    pdfDoc.value = doc
  }

  function setHighlights(slug: string, hl: Highlight[]) {
    highlightsBySlug.value = { ...highlightsBySlug.value, [slug]: hl }
  }

  function setReadingState(slug: string, rs: ReadingState | null) {
    readingStateBySlug.value = { ...readingStateBySlug.value, [slug]: rs }
  }

  async function saveHighlights() {
    const slug = activeSlug.value
    if (!slug) return
    try {
      await invoke('save_highlights', { slug, highlights: highlightsBySlug.value[slug] ?? [] })
    } catch (e) {
      console.error('Failed to save highlights:', e)
    }
  }

  // add/update/remove act on the active tab — only the visible viewer and the
  // sidebar (which mirror the active tab) ever mutate highlights.
  function addHighlight(h: Highlight) {
    const slug = activeSlug.value
    if (!slug) return
    setHighlights(slug, [...(highlightsBySlug.value[slug] ?? []), h])
    saveHighlights()
  }

  function updateHighlight(
    id: string,
    changes: Partial<Pick<Highlight, 'note' | 'color' | 'style' | 'start_offset' | 'end_offset'>>,
  ) {
    const slug = activeSlug.value
    if (!slug) return
    setHighlights(slug, (highlightsBySlug.value[slug] ?? []).map(h => h.id === id ? { ...h, ...changes } : h))
    saveHighlights()
  }

  function removeHighlight(id: string) {
    const slug = activeSlug.value
    if (!slug) return
    setHighlights(slug, (highlightsBySlug.value[slug] ?? []).filter(h => h.id !== id))
    saveHighlights()
  }

  function jumpToHighlight(id: string) {
    scrollToHighlightId.value = id
  }

  async function persistReadingState(rs: ReadingState) {
    const slug = activeSlug.value
    if (!slug) return
    setReadingState(slug, rs)
    try {
      await invoke('update_reading_state', { slug, readingState: rs })
    } catch (e) {
      console.error('Failed to save reading state:', e)
    }
  }

  return {
    tabs,
    activeSlug,
    openSlug,
    openTitle,
    pdfDoc,
    highlights,
    readingState,
    highlightsFor,
    readingStateFor,
    discardTabState,
    scrollToHighlightId,
    pendingPageJump,
    openPaper,
    replacePaperSlug,
    pruneStaleTabs,
    switchTab,
    closeTab,
    closePaper,
    showList,
    reorderTabs,
    saveTabs,
    loadTabs,
    setPdfDoc,
    setHighlights,
    setReadingState,
    saveHighlights,
    addHighlight,
    updateHighlight,
    removeHighlight,
    jumpToHighlight,
    persistReadingState,
  }
})
