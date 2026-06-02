import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useLibraryStore } from './library'
import type { Collection, CollectionsFile, PaperIndexEntry } from '../types'

export const useCollectionsStore = defineStore('collections', () => {
  const file = ref<CollectionsFile>({ collections: [], assignments: [] })
  const library = useLibraryStore()

  const topLevel = computed(() =>
    file.value.collections
      .filter(c => !c.parent_id)
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }))
  )

  function childrenOf(parentId: string): Collection[] {
    return file.value.collections
      .filter(c => c.parent_id === parentId)
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }))
  }

  function collectionById(id: string): Collection | undefined {
    return file.value.collections.find(c => c.id === id)
  }

  function paperIdsInCollection(collectionId: string): string[] {
    return file.value.assignments
      .filter(a => a.collection_id === collectionId)
      .map(a => a.paper_id)
  }

  const paperCountsByCollection = computed(() => {
    const childMap = new Map<string, string[]>()
    const directPaperIds = new Map<string, Set<string>>()
    const existingPaperIds = new Set(library.papers.map(p => p.id))

    for (const col of file.value.collections) {
      if (col.parent_id) {
        const children = childMap.get(col.parent_id) ?? []
        children.push(col.id)
        childMap.set(col.parent_id, children)
      }
    }

    for (const assignment of file.value.assignments) {
      if (!existingPaperIds.has(assignment.paper_id)) continue
      const ids = directPaperIds.get(assignment.collection_id) ?? new Set<string>()
      ids.add(assignment.paper_id)
      directPaperIds.set(assignment.collection_id, ids)
    }

    const memo = new Map<string, Set<string>>()

    function collectPaperIds(collectionId: string, visiting = new Set<string>()): Set<string> {
      if (memo.has(collectionId)) return new Set(memo.get(collectionId))
      if (visiting.has(collectionId)) return new Set()

      visiting.add(collectionId)
      const ids = new Set(directPaperIds.get(collectionId) ?? [])
      for (const childId of childMap.get(collectionId) ?? []) {
        for (const paperId of collectPaperIds(childId, visiting)) {
          ids.add(paperId)
        }
      }
      visiting.delete(collectionId)

      memo.set(collectionId, ids)
      return new Set(ids)
    }

    const counts: Record<string, number> = {}
    for (const col of file.value.collections) {
      counts[col.id] = collectPaperIds(col.id).size
    }
    return counts
  })

  function collectionPaperCount(collectionId: string): number {
    return paperCountsByCollection.value[collectionId] ?? 0
  }

  async function load() {
    try {
      file.value = await invoke<CollectionsFile>('get_collections')
    } catch (e) {
      console.error('Failed to load collections:', e)
    }
  }

  async function create(name: string, parentId?: string): Promise<Collection | null> {
    try {
      const col = await invoke<Collection>('create_collection', {
        name,
        parentId: parentId ?? null,
      })
      file.value.collections.push(col)
      return col
    } catch (e) {
      console.error('create_collection:', e)
      return null
    }
  }

  async function rename(id: string, newName: string) {
    try {
      await invoke('rename_collection', { id, newName })
      const col = file.value.collections.find(c => c.id === id)
      if (col) col.name = newName
    } catch (e) {
      console.error('rename_collection:', e)
    }
  }

  async function setEmoji(id: string, emoji: string | null) {
    try {
      const value = emoji?.trim() || null
      await invoke('set_collection_emoji', { id, emoji: value })
      const col = file.value.collections.find(c => c.id === id)
      if (col) col.emoji = value ?? undefined
    } catch (e) {
      console.error('set_collection_emoji:', e)
    }
  }

  async function remove(id: string) {
    try {
      await invoke('delete_collection', { id })
      // Remove cascade: the backend handles it; reload to be in sync.
      await load()
    } catch (e) {
      console.error('delete_collection:', e)
    }
  }

  async function addPaper(paperId: string, collectionId: string) {
    try {
      await invoke('add_paper_to_collection', { paperId, collectionId })
      const exists = file.value.assignments.some(
        a => a.paper_id === paperId && a.collection_id === collectionId
      )
      if (!exists) {
        file.value.assignments.push({ paper_id: paperId, collection_id: collectionId })
      }
    } catch (e) {
      console.error('add_paper_to_collection:', e)
    }
  }

  async function removePaper(paperId: string, collectionId: string) {
    try {
      await invoke('remove_paper_from_collection', { paperId, collectionId })
      file.value.assignments = file.value.assignments.filter(
        a => !(a.paper_id === paperId && a.collection_id === collectionId)
      )
    } catch (e) {
      console.error('remove_paper_from_collection:', e)
    }
  }

  async function movePaper(paperId: string, targetCollectionId: string) {
    try {
      await invoke('move_paper_to_collection', { paperId, collectionId: targetCollectionId })
      file.value.assignments = file.value.assignments.filter(a => a.paper_id !== paperId)
      file.value.assignments.push({ paper_id: paperId, collection_id: targetCollectionId })
    } catch (e) {
      console.error('move_paper_to_collection:', e)
    }
  }

  async function listPapersInCollection(collectionId: string): Promise<PaperIndexEntry[]> {
    try {
      return await invoke<PaperIndexEntry[]>('list_papers_in_collection', { collectionId })
    } catch (e) {
      console.error('list_papers_in_collection:', e)
      return []
    }
  }

  return {
    file,
    topLevel,
    childrenOf,
    collectionById,
    paperIdsInCollection,
    paperCountsByCollection,
    collectionPaperCount,
    load,
    create,
    rename,
    setEmoji,
    remove,
    addPaper,
    removePaper,
    movePaper,
    listPapersInCollection,
  }
})
