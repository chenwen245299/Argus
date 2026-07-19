import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

/** A named reference list the user is citing in one manuscript. Membership only —
 *  papers stay in their library location and may belong to several lists. */
export interface WritingList {
  id: string
  name: string
  created_at: string
  paper_ids: string[]
}

// Module-level reactive state, shared between the sidebar and the writing view
// (mirrors the snippetLibrary store's ref-based pattern).
export const lists = ref<WritingList[]>([])
/** Currently selected list; null means the "All papers" view. */
export const activeListId = ref<string | null>(null)

export async function loadLists() {
  try {
    const file = await invoke<{ lists: WritingList[] }>('list_writing_lists')
    lists.value = file.lists ?? []
    // Drop a stale selection if that list was removed elsewhere.
    if (activeListId.value && !lists.value.some(l => l.id === activeListId.value)) {
      activeListId.value = null
    }
  } catch {
    lists.value = []
  }
}

export async function createList(name: string): Promise<WritingList | null> {
  const trimmed = name.trim()
  if (!trimmed) return null
  const list = await invoke<WritingList>('create_writing_list', { name: trimmed })
  await loadLists()
  return list
}

export async function renameList(id: string, name: string) {
  const trimmed = name.trim()
  if (!trimmed) return
  await invoke('rename_writing_list', { id, name: trimmed })
  await loadLists()
}

export async function deleteList(id: string) {
  await invoke('delete_writing_list', { id })
  if (activeListId.value === id) activeListId.value = null
  await loadLists()
}

export async function addPapersToList(id: string, paperIds: string[]) {
  if (paperIds.length === 0) return
  await invoke('add_papers_to_writing_list', { id, paperIds })
  await loadLists()
}

export async function removePaperFromList(id: string, paperId: string) {
  await invoke('remove_paper_from_writing_list', { id, paperId })
  await loadLists()
}

export function listById(id: string | null): WritingList | undefined {
  return id ? lists.value.find(l => l.id === id) : undefined
}
