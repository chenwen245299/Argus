import { invoke } from '@tauri-apps/api/core'
import { ref } from 'vue'
import { useLibraryStore } from '../stores/library'
import { useCollectionsStore } from '../stores/collections'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import type { LibraryNoteSnapshot } from '../types'
import { i18n } from '../i18n'

export const creatingLibraryNote = ref(false)
export async function createLibraryNote(collectionId: string | null) {
  if (creatingLibraryNote.value) return
  const library = useLibraryStore(), collections = useCollectionsStore()
  const root = library.currentPath
  if (!root) return
  creatingLibraryNote.value = true
  try {
    const baseTitle = i18n.global.t('libraryNote.defaultTitle')
    const titles = new Set(library.papers.filter(p => p.file_type === 'md').map(p => p.title))
    let title = baseTitle, number = 2
    while (titles.has(title)) title = i18n.global.t('libraryNote.numberedTitle', { n: number++ })
    const note = await invoke<LibraryNoteSnapshot>('create_library_note', {
      collectionId, title, content: '', expectedRoot: root,
    })
    if (library.currentPath !== root) return
    library.applyCommittedPaper({
      slug: note.slug, id: note.id, title: note.title, authors: [], tags: [], added_at: note.createdAt,
      reading_status: 'unread', file_type: 'md', import_source: 'file', related_ids: [], has_bibtex: false,
      status: { text_extracted: false, metadata_fetched: false, ai_summary_done: false, vectorized: false, last_updated: note.createdAt },
    }, root)
    collections.applyCommittedPaperAssignment(note.id, note.collectionId, root)
    useSelectionStore().selectPaper(note.slug)
    useReaderStore().openPaper(note.slug, note.title, 'md')
    // The committed note is usable now. Reconcile unrelated changes separately;
    // slow scans must never hold the button disabled or delay opening the editor.
    void Promise.all([library.refresh(), collections.load()]).catch(error => console.error('Refresh after note creation:', error))
    return note
  } finally { creatingLibraryNote.value = false }
}
