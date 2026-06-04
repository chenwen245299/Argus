import { ref } from 'vue'
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

const LIBRARIES_KEY = 'argus:snippet-libraries'
const SNIPPETS_KEY = 'argus:snippets'

function nowIso() {
  return new Date().toISOString()
}

function newId() {
  return crypto.randomUUID()
}

function loadLibraries(): SnippetLibrary[] {
  try {
    const raw = localStorage.getItem(LIBRARIES_KEY)
    if (!raw) return []
    return JSON.parse(raw) as SnippetLibrary[]
  } catch {
    return []
  }
}

function saveLibraries(libs: SnippetLibrary[]) {
  try {
    localStorage.setItem(LIBRARIES_KEY, JSON.stringify(libs))
  } catch {}
}

function loadSnippets(): Snippet[] {
  try {
    const raw = localStorage.getItem(SNIPPETS_KEY)
    if (!raw) return []
    return JSON.parse(raw) as Snippet[]
  } catch {
    return []
  }
}

function saveSnippets(snips: Snippet[]) {
  try {
    localStorage.setItem(SNIPPETS_KEY, JSON.stringify(snips))
  } catch {}
}

export const libraries = ref<SnippetLibrary[]>(loadLibraries())
export const snippets = ref<Snippet[]>(loadSnippets())

export function createLibrary(name: string, emoji?: string): SnippetLibrary {
  const lib: SnippetLibrary = { id: newId(), name: name.trim(), emoji, createdAt: nowIso() }
  libraries.value = [...libraries.value, lib]
  saveLibraries(libraries.value)
  return lib
}

export function renameLibrary(id: string, name: string) {
  libraries.value = libraries.value.map(l => l.id === id ? { ...l, name: name.trim() } : l)
  saveLibraries(libraries.value)
}

export function deleteLibrary(id: string) {
  libraries.value = libraries.value.filter(l => l.id !== id)
  saveLibraries(libraries.value)
  snippets.value = snippets.value.filter(s => s.libraryId !== id)
  saveSnippets(snippets.value)
}

export function addSnippet(data: {
  libraryId: string
  text: string
  tags: string[]
  note: string
  paperId: string
  paperTitle: string
  page: number
  color?: string
}): Snippet {
  const s: Snippet = { id: newId(), ...data, createdAt: nowIso() }
  snippets.value = [...snippets.value, s]
  saveSnippets(snippets.value)
  return s
}

export function deleteSnippet(id: string) {
  snippets.value = snippets.value.filter(s => s.id !== id)
  saveSnippets(snippets.value)
}

export function updateSnippet(id: string, changes: Partial<Pick<Snippet, 'tags' | 'note'>>) {
  snippets.value = snippets.value.map(s => s.id === id ? { ...s, ...changes } : s)
  saveSnippets(snippets.value)
}

export function snippetsForLibrary(libraryId: string): Snippet[] {
  return snippets.value.filter(s => s.libraryId === libraryId)
}

// Global signal to open the "Add to Snippet Library" modal from anywhere
export const pendingSnippet = ref<PendingSnippet | null>(null)

export function openAddSnippetModal(data: PendingSnippet) {
  pendingSnippet.value = data
}
