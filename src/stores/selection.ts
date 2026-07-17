import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { NavItem, SearchHit } from '../types'

export const useSelectionStore = defineStore('selection', () => {
  const selectedSlug = ref<string | null>(null)
  // Multi-selection (Cmd/Ctrl/Shift + click). Always contains `selectedSlug`
  // while anything is selected; `selectedSlug` is the "primary" row that drives
  // the preview panel.
  const selectedSlugs = ref<string[]>([])
  const activeNav = ref<NavItem>('all')
  const tagFilter = ref<string | null>(null)
  const activeCollectionId = ref<string | null>(null)
  const highlightedCollectionId = ref<string | null>(null)
  const navSelectionSeq = ref(0)

  // Search state
  const searchQuery = ref('')
  const searchResults = ref<SearchHit[]>([])

  function selectPaper(slug: string | null) {
    // Selecting a row only previews it — it is not "opening/reading", so it must
    // not affect "最近阅读" order (that's recorded in reader.openPaper).
    selectedSlug.value = slug
    selectedSlugs.value = slug ? [slug] : []
  }

  // Cmd/Ctrl + click — toggle one row in/out of the multi-selection.
  function togglePaper(slug: string) {
    const set = new Set(selectedSlugs.value)
    if (set.has(slug)) {
      set.delete(slug)
      selectedSlugs.value = [...set]
      if (selectedSlug.value === slug) {
        selectedSlug.value = selectedSlugs.value[selectedSlugs.value.length - 1] ?? null
      }
    } else {
      set.add(slug)
      selectedSlugs.value = [...set]
      selectedSlug.value = slug
    }
  }

  // Shift + click — replace the selection with an explicit range (the caller
  // resolves which slugs fall between the anchor and the clicked row).
  function selectRange(slugs: string[], primary: string) {
    selectedSlugs.value = [...slugs]
    selectedSlug.value = primary
  }

  function selectNav(item: NavItem) {
    navSelectionSeq.value += 1
    activeNav.value = item
    if (item === 'all' || item === 'recent' || item === 'inbox') {
      tagFilter.value = null
      activeCollectionId.value = null
      highlightedCollectionId.value = null
    } else if (item.startsWith('tag:')) {
      tagFilter.value = item.slice(4)
      activeCollectionId.value = null
      highlightedCollectionId.value = null
    } else if (item.startsWith('collection:')) {
      activeCollectionId.value = item.slice('collection:'.length)
      highlightedCollectionId.value = activeCollectionId.value
      tagFilter.value = null
    } else if (item === 'search') {
      tagFilter.value = null
      activeCollectionId.value = null
      highlightedCollectionId.value = null
    }
    selectedSlug.value = null
    selectedSlugs.value = []
  }

  function toggleTagFilter(tag: string) {
    navSelectionSeq.value += 1
    if (activeNav.value === 'search' || activeNav.value === 'inbox') {
      activeNav.value = 'all'
      activeCollectionId.value = null
    }
    highlightedCollectionId.value = null
    tagFilter.value = tagFilter.value === tag ? null : tag
    selectedSlug.value = null
    selectedSlugs.value = []
  }

  function setSearchResults(query: string, results: SearchHit[]) {
    navSelectionSeq.value += 1
    searchQuery.value = query
    searchResults.value = results
    activeNav.value = 'search'
    selectedSlug.value = null
    selectedSlugs.value = []
    tagFilter.value = null
    activeCollectionId.value = null
    highlightedCollectionId.value = null
  }

  function clearSearch() {
    searchQuery.value = ''
    searchResults.value = []
    if (activeNav.value === 'search') {
      activeNav.value = 'all'
    }
  }

  // Update the sidebar highlight only — does NOT change the center view or selectedSlug
  function highlightCollection(collectionId: string | null) {
    highlightedCollectionId.value = collectionId
  }

  return {
    selectedSlug,
    selectedSlugs,
    activeNav,
    navSelectionSeq,
    tagFilter,
    activeCollectionId,
    highlightedCollectionId,
    searchQuery,
    searchResults,
    selectPaper,
    togglePaper,
    selectRange,
    selectNav,
    toggleTagFilter,
    setSearchResults,
    clearSearch,
    highlightCollection,
  }
})
