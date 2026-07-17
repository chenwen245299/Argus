import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { LibraryConfig, PaperIndexEntry } from '../types'
import { useReaderStore } from './reader'
import { useRanksStore } from './ranks'

export const useLibraryStore = defineStore('library', () => {
  const currentPath = ref<string | null>(null)
  const papers = ref<PaperIndexEntry[]>([])
  const isCheckingLibrary = ref(true)
  const isRestoringLibrary = ref(false)
  const isLoading = ref(false)
  const isRefreshing = ref(false)
  const error = ref<string | null>(null)

  // The in-flight refresh promise (so concurrent callers can await the same run),
  // plus a flag requesting a trailing re-run for requests that arrived mid-refresh.
  let refreshPromise: Promise<void> | null = null
  let refreshPending = false

  const allTags = computed(() => {
    const set = new Set<string>()
    papers.value.forEach(p => p.tags?.forEach(t => set.add(t)))
    return [...set].sort()
  })

  async function initialize() {
    isCheckingLibrary.value = true
    isRestoringLibrary.value = false
    try {
      const path = await invoke<string | null>('get_current_library')
      isCheckingLibrary.value = false
      if (path) {
        isRestoringLibrary.value = true
        await openLibrary(path)
      }
    } catch (e) {
      console.error('Failed to restore last library:', e)
    } finally {
      isCheckingLibrary.value = false
      isRestoringLibrary.value = false
    }
  }

  async function pickAndOpen() {
    try {
      const path = await invoke<string>('pick_library_folder')
      await openLibrary(path)
    } catch (e: unknown) {
      const msg = e as string
      if (msg !== 'cancelled') error.value = msg
    }
  }

  async function openLibrary(path: string) {
    isLoading.value = true
    error.value = null
    try {
      await invoke<LibraryConfig>('open_library', { root: path })
      currentPath.value = path

      // Load the library-wide venue→rank cache so list/info badges render
      // instantly without re-querying easyScholar.
      useRanksStore().load()

      // Phase 1: instant display from index.json cache (best-effort — silently skipped if unavailable)
      try {
        const cached = await invoke<PaperIndexEntry[]>('load_library_cache')
        if (cached.length > 0) {
          papers.value = cached
          isLoading.value = false  // UI usable immediately
        }
      } catch {
        // No cache yet or command unavailable — Phase 2 will do a full scan
      }

      // Phase 2: incremental background scan (always runs, updates any changes)
      _backgroundScan()
    } catch (e) {
      error.value = String(e)
      isLoading.value = false
    }
  }

  async function _backgroundScan() {
    if (!currentPath.value) return
    isRefreshing.value = true
    try {
      const fresh = await invoke<PaperIndexEntry[]>('scan_library')
      papers.value = fresh
      const reader = useReaderStore()
      const slugs = new Set(fresh.map(p => p.slug))
      reader.pruneStaleTabs(slugs)
    } catch (e) {
      console.error('[library] background scan failed:', e)
    } finally {
      isRefreshing.value = false
      isLoading.value = false
    }
  }

  function removePaper(slug: string) {
    papers.value = papers.value.filter(p => p.slug !== slug)
  }

  // ── Related papers (manual, bidirectional links) ──────────────────────────
  // Which paper's "related" popover is open, and where to anchor it. `anchor`
  // null = center on screen (right-click entry); a point = anchor near it
  // (toolbar-button entry).
  const relatedPopover = ref<{ slug: string; anchor: { x: number; y: number } | null } | null>(null)

  function openRelatedPopover(slug: string, anchor: { x: number; y: number } | null = null) {
    relatedPopover.value = { slug, anchor }
  }
  function closeRelatedPopover() {
    relatedPopover.value = null
  }

  // ── Citation graph (Semantic Scholar references) ──────────────────────────
  // Which paper's citation-graph modal is open (null = closed). `anchor` is the
  // triggering button's screen center, used for the open/close scale animation.
  const citationGraph = ref<{ slug: string; anchor: { x: number; y: number } | null } | null>(null)
  function openCitationGraph(slug: string, anchor: { x: number; y: number } | null = null) {
    citationGraph.value = { slug, anchor }
  }
  function closeCitationGraph() {
    citationGraph.value = null
  }

  /** Resolve a paper's related ids to their index entries (skips missing ones). */
  function relatedEntriesFor(slug: string): PaperIndexEntry[] {
    const self = papers.value.find(p => p.slug === slug)
    if (!self?.related_ids?.length) return []
    return self.related_ids
      .map(id => papers.value.find(p => p.id === id))
      .filter((p): p is PaperIndexEntry => !!p)
  }

  async function linkRelated(slugA: string, slugB: string) {
    if (slugA === slugB) return
    await invoke('add_related_paper', { slugA, slugB })
    const a = papers.value.find(p => p.slug === slugA)
    const b = papers.value.find(p => p.slug === slugB)
    if (a && b && a.id !== b.id) {
      if (!a.related_ids?.includes(b.id)) a.related_ids = [...(a.related_ids ?? []), b.id]
      if (!b.related_ids?.includes(a.id)) b.related_ids = [...(b.related_ids ?? []), a.id]
    }
  }

  async function unlinkRelated(slugA: string, slugB: string) {
    await invoke('remove_related_paper', { slugA, slugB })
    const a = papers.value.find(p => p.slug === slugA)
    const b = papers.value.find(p => p.slug === slugB)
    if (a && b) {
      if (a.related_ids) a.related_ids = a.related_ids.filter(id => id !== b.id)
      if (b.related_ids) b.related_ids = b.related_ids.filter(id => id !== a.id)
    }
  }

  async function _doRefresh(): Promise<void> {
    isRefreshing.value = true
    try {
      const fresh = await invoke<PaperIndexEntry[]>('scan_library')
      papers.value = fresh
      const reader = useReaderStore()
      const slugs = new Set(fresh.map(p => p.slug))
      reader.pruneStaleTabs(slugs)
    } catch (e) {
      error.value = String(e)
    } finally {
      isRefreshing.value = false
      isLoading.value = false
    }
  }

  async function refresh(): Promise<void> {
    if (!currentPath.value) return
    // A refresh is already running: mark a trailing re-run (so any state that
    // changed during the current scan gets picked up) and share its promise so
    // every caller awaits a scan that started at or after their call.
    if (refreshPromise) {
      refreshPending = true
      return refreshPromise
    }

    refreshPromise = (async () => {
      try {
        await _doRefresh()
        // Drain any requests that arrived while we were scanning, running at
        // most one trailing pass (which itself absorbs further requests).
        while (refreshPending) {
          refreshPending = false
          await _doRefresh()
        }
      } finally {
        refreshPromise = null
      }
    })()

    return refreshPromise
  }

  return {
    currentPath,
    papers,
    isCheckingLibrary,
    isRestoringLibrary,
    isLoading,
    isRefreshing,
    error,
    allTags,
    initialize,
    pickAndOpen,
    openLibrary,
    removePaper,
    refresh,
    relatedPopover,
    openRelatedPopover,
    closeRelatedPopover,
    relatedEntriesFor,
    linkRelated,
    unlinkRelated,
    citationGraph,
    openCitationGraph,
    closeCitationGraph,
  }
})
