import { defineStore } from 'pinia'
import { ref, computed, shallowRef } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Highlight, ReadingState } from '../types'
import type { PDFDocumentProxy } from 'pdfjs-dist'

export interface Tab {
  slug: string
  title: string
}

export const useReaderStore = defineStore('reader', () => {
  const tabs      = ref<Tab[]>([])
  const activeSlug = ref<string | null>(null)

  // Backward-compat computed props (PdfViewer uses these)
  const openSlug  = computed(() => activeSlug.value)
  const openTitle = computed(() => tabs.value.find(t => t.slug === activeSlug.value)?.title ?? '')

  // Per-active-tab state (reset on tab switch; PdfViewer reloads from disk)
  const pdfDoc              = shallowRef<PDFDocumentProxy | null>(null)
  const highlights          = ref<Highlight[]>([])
  const readingState        = ref<ReadingState | null>(null)
  const scrollToHighlightId = ref<string | null>(null)
  const pendingPageJump     = ref<number | null>(null)

  function _resetTabState() {
    pdfDoc.value              = null
    highlights.value          = []
    readingState.value        = null
    scrollToHighlightId.value = null
    pendingPageJump.value     = null
  }

  function openPaper(slug: string, title: string) {
    const existing = tabs.value.find(t => t.slug === slug)
    if (!existing) {
      tabs.value.push({ slug, title })
    } else {
      existing.title = title  // update title in case it changed
    }
    if (activeSlug.value !== slug) {
      activeSlug.value = slug
      _resetTabState()
    }
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
    activeSlug.value = slug
    _resetTabState()
  }

  function closeTab(slug: string) {
    const idx = tabs.value.findIndex(t => t.slug === slug)
    if (idx === -1) return
    tabs.value.splice(idx, 1)
    if (activeSlug.value === slug) {
      const next = tabs.value[Math.min(idx, tabs.value.length - 1)]
      activeSlug.value = next?.slug ?? null
      _resetTabState()
    }
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
    tabs.value = tabs.value.filter(t => validSlugs.has(t.slug))
    if (tabs.value.length !== before && activeSlug.value && !validSlugs.has(activeSlug.value)) {
      activeSlug.value = tabs.value[0]?.slug ?? null
      _resetTabState()
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

  function setPdfDoc(doc: PDFDocumentProxy) {
    pdfDoc.value = doc
  }

  function setHighlights(hl: Highlight[]) {
    highlights.value = hl
  }

  function setReadingState(rs: ReadingState | null) {
    readingState.value = rs
  }

  async function saveHighlights() {
    const slug = activeSlug.value
    if (!slug) return
    try {
      await invoke('save_highlights', { slug, highlights: highlights.value })
    } catch (e) {
      console.error('Failed to save highlights:', e)
    }
  }

  function addHighlight(h: Highlight) {
    highlights.value = [...highlights.value, h]
    saveHighlights()
  }

  function updateHighlight(id: string, changes: Partial<Pick<Highlight, 'note' | 'color' | 'style'>>) {
    highlights.value = highlights.value.map(h => h.id === id ? { ...h, ...changes } : h)
    saveHighlights()
  }

  function removeHighlight(id: string) {
    highlights.value = highlights.value.filter(h => h.id !== id)
    saveHighlights()
  }

  function jumpToHighlight(id: string) {
    scrollToHighlightId.value = id
  }

  async function persistReadingState(rs: ReadingState) {
    readingState.value = rs
    const slug = activeSlug.value
    if (!slug) return
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
