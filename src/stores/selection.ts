import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { NavItem, SearchHit } from '../types'
import { recordPaperAccess } from '../utils/recentPapers'

export const useSelectionStore = defineStore('selection', () => {
  const selectedSlug = ref<string | null>(null)
  const activeNav = ref<NavItem>('all')
  const tagFilter = ref<string | null>(null)
  const activeCollectionId = ref<string | null>(null)
  const navSelectionSeq = ref(0)

  // Search state
  const searchQuery = ref('')
  const searchResults = ref<SearchHit[]>([])

  function selectPaper(slug: string | null) {
    recordPaperAccess(slug)
    selectedSlug.value = slug
  }

  function selectNav(item: NavItem) {
    navSelectionSeq.value += 1
    activeNav.value = item
    if (item === 'all' || item === 'recent' || item === 'inbox') {
      tagFilter.value = null
      activeCollectionId.value = null
    } else if (item.startsWith('tag:')) {
      tagFilter.value = item.slice(4)
      activeCollectionId.value = null
    } else if (item.startsWith('collection:')) {
      activeCollectionId.value = item.slice('collection:'.length)
      tagFilter.value = null
    } else if (item === 'search') {
      tagFilter.value = null
      activeCollectionId.value = null
    }
    selectedSlug.value = null
  }

  function toggleTagFilter(tag: string) {
    navSelectionSeq.value += 1
    if (activeNav.value === 'search' || activeNav.value === 'inbox') {
      activeNav.value = 'all'
      activeCollectionId.value = null
    }
    tagFilter.value = tagFilter.value === tag ? null : tag
    selectedSlug.value = null
  }

  function setSearchResults(query: string, results: SearchHit[]) {
    navSelectionSeq.value += 1
    searchQuery.value = query
    searchResults.value = results
    activeNav.value = 'search'
    selectedSlug.value = null
    tagFilter.value = null
    activeCollectionId.value = null
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
    if (collectionId) {
      activeNav.value = `collection:${collectionId}` as NavItem
      activeCollectionId.value = collectionId
    } else {
      activeNav.value = 'all'
      activeCollectionId.value = null
    }
  }

  return {
    selectedSlug,
    activeNav,
    navSelectionSeq,
    tagFilter,
    activeCollectionId,
    searchQuery,
    searchResults,
    selectPaper,
    selectNav,
    toggleTagFilter,
    setSearchResults,
    clearSearch,
    highlightCollection,
  }
})
