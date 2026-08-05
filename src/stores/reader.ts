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
  /** Chrome-style tab group this tab belongs to. Absent = ungrouped. */
  groupId?: string
}

/** Chrome's tab-group palette. */
export const TAB_GROUP_COLORS = [
  'grey', 'blue', 'red', 'yellow', 'green', 'pink', 'purple', 'cyan', 'orange',
] as const
export type TabGroupColor = (typeof TAB_GROUP_COLORS)[number]

export interface TabGroup {
  id: string
  /** Empty name renders as a bare colour dot, like Chrome. */
  name: string
  color: TabGroupColor
  collapsed: boolean
}

interface PersistedTabs {
  tabs: Tab[]
  activeSlug: string | null
  groups: TabGroup[]
}

export const useReaderStore = defineStore('reader', () => {
  const tabs      = ref<Tab[]>([])
  const activeSlug = ref<string | null>(null)
  const tabGroups = ref<TabGroup[]>([])

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
  let tabSaveChain: Promise<unknown> = Promise.resolve()

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

  // ── Tab groups ──────────────────────────────────────────────────────────────
  // Two invariants, both borrowed from Chrome, are maintained by
  // `normalizeGroups()` after every mutation:
  //   1. a group's members are contiguous in `tabs`, anchored where its first
  //      member sits — so a group can be drawn as one continuous sleeve;
  //   2. a group with no members ceases to exist.

  function groupById(id: string | null | undefined): TabGroup | null {
    return id ? tabGroups.value.find(g => g.id === id) ?? null : null
  }

  function tabsInGroup(id: string): Tab[] {
    return tabs.value.filter(t => t.groupId === id)
  }

  function normalizeGroups() {
    const known = new Set(tabGroups.value.map(g => g.id))
    // A groupId with no group behind it (stale persisted state) means ungrouped.
    for (const tab of tabs.value) {
      if (tab.groupId && !known.has(tab.groupId)) delete tab.groupId
    }
    const emitted = new Set<string>()
    const ordered: Tab[] = []
    for (const tab of tabs.value) {
      if (!tab.groupId) { ordered.push(tab); continue }
      if (emitted.has(tab.groupId)) continue  // already pulled in with its group
      emitted.add(tab.groupId)
      ordered.push(...tabs.value.filter(t => t.groupId === tab.groupId))
    }
    if (ordered.some((t, i) => t !== tabs.value[i])) tabs.value = ordered
    if (tabGroups.value.some(g => !emitted.has(g.id))) {
      tabGroups.value = tabGroups.value.filter(g => emitted.has(g.id))
    }
  }

  /** Cycle the palette so consecutive new groups don't land on the same colour. */
  function nextGroupColor(): TabGroupColor {
    const used = new Set(tabGroups.value.map(g => g.color))
    return TAB_GROUP_COLORS.find(c => !used.has(c)) ?? TAB_GROUP_COLORS[tabGroups.value.length % TAB_GROUP_COLORS.length]
  }

  function createTabGroup(slugs: string[], name = '', color?: TabGroupColor): string | null {
    const members = slugs.filter(s => tabs.value.some(t => t.slug === s))
    if (!members.length) return null
    const group: TabGroup = {
      id: `tg_${crypto.randomUUID()}`,
      name,
      color: color ?? nextGroupColor(),
      collapsed: false,
    }
    tabGroups.value = [...tabGroups.value, group]
    for (const tab of tabs.value) {
      if (members.includes(tab.slug)) tab.groupId = group.id
    }
    normalizeGroups()
    return group.id
  }

  function setTabGroup(slug: string, groupId: string | null) {
    const tab = tabs.value.find(t => t.slug === slug)
    if (!tab) return
    if (groupId && !groupById(groupId)) return
    if (groupId) {
      tab.groupId = groupId
      // Joining a collapsed group would make the tab vanish; open it instead.
      const group = groupById(groupId)
      if (group?.collapsed && activeSlug.value === slug) group.collapsed = false
    } else {
      delete tab.groupId
    }
    normalizeGroups()
  }

  function renameTabGroup(id: string, name: string) {
    const group = groupById(id)
    if (group) group.name = name
  }

  function setTabGroupColor(id: string, color: TabGroupColor) {
    const group = groupById(id)
    if (group) group.color = color
  }

  function setTabGroupCollapsed(id: string, collapsed: boolean) {
    const group = groupById(id)
    if (!group || group.collapsed === collapsed) return
    group.collapsed = collapsed
    // Collapsing the group holding the active tab would hide what's on screen,
    // so hand activation to the nearest tab outside it (Chrome does the same).
    if (collapsed && activeSlug.value) {
      const active = tabs.value.find(t => t.slug === activeSlug.value)
      if (active?.groupId === id) {
        const idx = tabs.value.indexOf(active)
        const outside = [...tabs.value.slice(idx + 1), ...tabs.value.slice(0, idx).reverse()]
          .find(t => t.groupId !== id)
        activeSlug.value = outside?.slug ?? null
      }
    }
  }

  function toggleTabGroupCollapsed(id: string) {
    setTabGroupCollapsed(id, !groupById(id)?.collapsed)
  }

  /** Dissolve the group, keeping its tabs open and in place. */
  function ungroupTabs(id: string) {
    for (const tab of tabs.value) {
      if (tab.groupId === id) delete tab.groupId
    }
    tabGroups.value = tabGroups.value.filter(g => g.id !== id)
    normalizeGroups()
  }

  function closeTabGroup(id: string) {
    for (const tab of tabsInGroup(id)) closeTab(tab.slug)
    normalizeGroups()
  }

  /** A tab becoming active must be visible, so open its group if collapsed. */
  function revealTab(slug: string) {
    const group = groupById(tabs.value.find(t => t.slug === slug)?.groupId)
    if (group?.collapsed) group.collapsed = false
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
    revealTab(slug)
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
    revealTab(slug)
  }

  function closeTab(slug: string) {
    const idx = tabs.value.findIndex(t => t.slug === slug)
    if (idx === -1) return
    tabs.value.splice(idx, 1)
    if (activeSlug.value === slug) {
      const next = tabs.value[Math.min(idx, tabs.value.length - 1)]
      activeSlug.value = next?.slug ?? null
      if (activeSlug.value) revealTab(activeSlug.value)
    }
    // Closing the last member retires the group.
    normalizeGroups()
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
    // Where a tab lands decides its group, as in Chrome: dropped between two
    // members of the same group it joins them; dropped anywhere else it comes
    // out — except when nudged around inside the group it already belongs to.
    const prev = arr[adjusted - 1]
    const next = arr[adjusted + 1]
    const between = prev?.groupId && prev.groupId === next?.groupId ? prev.groupId : undefined
    const stayingHome = item.groupId && (prev?.groupId === item.groupId || next?.groupId === item.groupId)
      ? item.groupId
      : undefined
    const target = between ?? stayingHome
    if (target) item.groupId = target
    else delete item.groupId
    tabs.value = arr
    normalizeGroups()
  }

  /** Move a whole group (chip + members) so it starts at `insertBefore`. */
  function reorderTabGroup(groupId: string, insertBefore: number) {
    const members = tabsInGroup(groupId)
    if (!members.length) return
    const rest = tabs.value.filter(t => t.groupId !== groupId)
    // `insertBefore` indexes the full list; translate it to the list without
    // the group by discounting the members that sit before it.
    const removedBefore = tabs.value.slice(0, insertBefore).filter(t => t.groupId === groupId).length
    const at = Math.max(0, Math.min(rest.length, insertBefore - removedBefore))
    rest.splice(at, 0, ...members)
    tabs.value = rest
    normalizeGroups()
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
    normalizeGroups()
  }

  function _tabKey(libraryPath: string) {
    return `argus:tabs:${libraryPath}`
  }

  function saveTabs(libraryPath: string) {
    try {
      const snapshot = JSON.parse(JSON.stringify({
        tabs: tabs.value,
        activeSlug: activeSlug.value,
        groups: tabGroups.value,
      })) as PersistedTabs
      localStorage.setItem(_tabKey(libraryPath), JSON.stringify(snapshot))
      tabSaveChain = tabSaveChain
        .catch(() => undefined)
        .then(() => invoke('patch_library_ui_state', {
          root: libraryPath,
          patch: { version: 1, tabs: snapshot },
        }))
        .catch(e => console.error('[reader] save ui_state tabs failed:', e))
    } catch {}
  }

  /** `groups` is absent in state written before tab groups existed. */
  function normalizePersistedGroups(value: unknown): TabGroup[] {
    if (!Array.isArray(value)) return []
    return value
      .filter((g): g is TabGroup => !!g && typeof g === 'object' && typeof (g as TabGroup).id === 'string')
      .map(g => ({
        id: g.id,
        name: typeof g.name === 'string' ? g.name : '',
        color: TAB_GROUP_COLORS.includes(g.color) ? g.color : 'grey',
        collapsed: !!g.collapsed,
      }))
  }

  function applyPersisted(state: PersistedTabs) {
    tabs.value = state.tabs
    activeSlug.value = state.activeSlug
    tabGroups.value = state.groups
    // Drops dangling groupIds and restores contiguity, so hand-edited or
    // pre-groups state still lands in a consistent shape.
    normalizeGroups()
    if (activeSlug.value) revealTab(activeSlug.value)
  }

  async function loadTabs(libraryPath: string) {
    let legacyState: PersistedTabs | null = null
    try {
      const raw = localStorage.getItem(_tabKey(libraryPath))
      if (raw) {
        const state = JSON.parse(raw) as Partial<PersistedTabs>
        legacyState = {
          tabs: Array.isArray(state.tabs) ? state.tabs : [],
          activeSlug: typeof state.activeSlug === 'string' ? state.activeSlug : null,
          groups: normalizePersistedGroups(state.groups),
        }
      }
    } catch {}

    try {
      const uiState = await invoke<Record<string, unknown>>('get_library_ui_state', { root: libraryPath })
      const state = uiState.tabs as Partial<PersistedTabs> | undefined
      if (state && Array.isArray(state.tabs)) {
        applyPersisted({
          tabs: state.tabs,
          activeSlug: typeof state.activeSlug === 'string' ? state.activeSlug : null,
          groups: normalizePersistedGroups(state.groups),
        })
        return
      }
      if (legacyState) {
        applyPersisted(legacyState)
        saveTabs(libraryPath)
        return
      }
    } catch (e) {
      console.error('[reader] load ui_state tabs failed:', e)
      if (legacyState) {
        applyPersisted(legacyState)
        return
      }
    }
    tabs.value = []
    activeSlug.value = null
    tabGroups.value = []
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

  /**
   * Re-read a paper's disk-backed state after it changed outside this app
   * (typically synced in from another machine).
   *
   * Only papers with an open tab are refreshed — everything else is read fresh
   * when its tab opens anyway. Highlights and reading state are both written
   * whole-file, so the sync already resolved them last-writer-wins on disk;
   * picking up the winner is strictly better than showing a version that no
   * longer exists anywhere. Notes are deliberately NOT touched: they can be open
   * in an editor with unsaved keystrokes, and clobbering those loses work that
   * was never on disk to begin with.
   */
  async function reloadFromDisk(slug: string) {
    if (!tabs.value.some(t => t.slug === slug)) return
    try {
      const [hl, rs] = await Promise.all([
        invoke<Highlight[]>('get_highlights', { slug }),
        invoke<ReadingState | null>('get_reading_state', { slug }),
      ])
      setHighlights(slug, hl)
      setReadingState(slug, rs)
    } catch (e) {
      console.error(`[reader] reload ${slug} after external change failed:`, e)
    }
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
    tabGroups,
    groupById,
    tabsInGroup,
    createTabGroup,
    setTabGroup,
    renameTabGroup,
    setTabGroupColor,
    setTabGroupCollapsed,
    toggleTabGroupCollapsed,
    ungroupTabs,
    closeTabGroup,
    reorderTabGroup,
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
    reloadFromDisk,
  }
})
