import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { Rect } from '../types'

export interface SnippetLibrary {
  id: string
  name: string
  emoji?: string
  createdAt: string
}

export interface Snippet {
  id: string
  libraryId: string
  text: string
  tags: string[]
  note: string
  paperId: string
  paperTitle: string
  page: number
  color?: string
  createdAt: string
}

export interface PendingSnippet {
  text: string
  paperId: string
  paperTitle: string
  page: number
  color?: string
  rects?: Rect[]
  pageIndex?: number
}

export interface PendingHighlight {
  rects: Rect[]
  pageIndex: number
  text: string
  color: string
}

// Signal PdfViewer to create a highlight after snippet is confirmed
export const pendingHighlight = ref<PendingHighlight | null>(null)

// Global signal to open the "Add to Snippet Library" modal from anywhere
export const pendingSnippet = ref<PendingSnippet | null>(null)

export const libraries = ref<SnippetLibrary[]>([])
export const snippets = ref<Snippet[]>([])

let migrationDone = false

export async function initSnippetStore() {
  // Migration only runs once per session; loadAll always runs to pick up the current library.
  if (!migrationDone) {
    migrationDone = true
    const LS_LIBS = 'argus:snippet-libraries'
    const LS_SNIPS = 'argus:snippets'
    const lsLibsRaw = localStorage.getItem(LS_LIBS)
    const lsSnipsRaw = localStorage.getItem(LS_SNIPS)
    if (lsLibsRaw) {
      try {
        const lsLibs: SnippetLibrary[] = JSON.parse(lsLibsRaw) ?? []
        const lsSnips: Snippet[] = lsSnipsRaw ? (JSON.parse(lsSnipsRaw) ?? []) : []
        const snippetsByLibrary: [string, Snippet[]][] = lsLibs.map(lib => [
          lib.id,
          lsSnips.filter(s => s.libraryId === lib.id),
        ])
        await invoke('migrate_snippets_from_localstorage', {
          libraries: lsLibs,
          snippetsByLibrary,
        })
        localStorage.removeItem(LS_LIBS)
        localStorage.removeItem(LS_SNIPS)
      } catch (e) {
        console.error('Snippet migration failed:', e)
      }
    }
  }

  await loadAll()
}

export async function loadAll() {
  try {
    const libs = await invoke<SnippetLibrary[]>('list_snippet_libraries')
    libraries.value = libs
    const allSnips: Snippet[] = []
    await Promise.all(
      libs.map(async lib => {
        const snips = await invoke<Snippet[]>('get_snippets', { libraryId: lib.id })
        allSnips.push(...snips)
      })
    )
    snippets.value = allSnips
  } catch (e) {
    console.error('Failed to load snippet libraries:', e)
  }
}

export async function createLibrary(name: string, emoji?: string): Promise<SnippetLibrary> {
  const lib = await invoke<SnippetLibrary>('create_snippet_library', { name, emoji: emoji ?? null })
  libraries.value = [...libraries.value, lib]
  return lib
}

export async function renameLibrary(id: string, name: string) {
  await invoke('rename_snippet_library', { id, name })
  libraries.value = libraries.value.map(l => l.id === id ? { ...l, name } : l)
}

export async function updateLibraryEmoji(id: string, emoji: string | null) {
  await invoke('update_snippet_library_emoji', { id, emoji })
  libraries.value = libraries.value.map(l => l.id === id ? { ...l, emoji: emoji ?? undefined } : l)
}

export async function deleteLibrary(id: string) {
  await invoke('delete_snippet_library', { id })
  libraries.value = libraries.value.filter(l => l.id !== id)
  snippets.value = snippets.value.filter(s => s.libraryId !== id)
}

export async function addSnippet(data: {
  libraryId: string
  text: string
  tags: string[]
  note: string
  paperId: string
  paperTitle: string
  page: number
  color?: string
}): Promise<Snippet> {
  const s = await invoke<Snippet>('add_snippet', { input: data })
  snippets.value = [...snippets.value, s]
  return s
}

export async function deleteSnippet(id: string) {
  const s = snippets.value.find(s => s.id === id)
  if (!s) return
  await invoke('delete_snippet', { libraryId: s.libraryId, id })
  snippets.value = snippets.value.filter(s => s.id !== id)
}

export async function updateSnippet(id: string, changes: Partial<Pick<Snippet, 'tags' | 'note'>>) {
  const s = snippets.value.find(s => s.id === id)
  if (!s) return
  await invoke('update_snippet', {
    libraryId: s.libraryId,
    id,
    tags: changes.tags ?? null,
    note: changes.note ?? null,
  })
  snippets.value = snippets.value.map(s => s.id === id ? { ...s, ...changes } : s)
}

export function snippetsForLibrary(libraryId: string): Snippet[] {
  return snippets.value.filter(s => s.libraryId === libraryId)
}

export function openAddSnippetModal(data: PendingSnippet) {
  pendingSnippet.value = data
}
