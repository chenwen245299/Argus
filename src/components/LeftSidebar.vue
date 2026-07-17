<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useLibraryStore } from '../stores/library'
import { useSelectionStore } from '../stores/selection'
import { useCollectionsStore } from '../stores/collections'
import { useCanvasStore } from '../stores/canvas'
import { useReaderStore } from '../stores/reader'
import { useRagStore } from '../stores/rag'
import CollectionNode from './CollectionNode.vue'
import type { CanvasIndexEntry, Collection, NavItem } from '../types'
import { updateStore } from '../stores/update'
import { filterRecentlyRead, recentPapersVersion } from '../utils/recentPapers'
import { ensureFluentIcons, fluentReady, fluentEmojiList, type FluentEmojiEntry } from '../utils/fluentEmoji'
import { libraries as snippetLibraries, snippets as allSnippets, createLibrary as createSnippetLibrary, deleteLibrary as deleteSnippetLibrary, renameLibrary as renameSnippetLibrary, loadAll as reloadSnippets } from '../stores/snippetLibrary'

const { t } = useI18n()
const library = useLibraryStore()
const selection = useSelectionStore()
const collectionsStore = useCollectionsStore()
const canvasStore = useCanvasStore()
const readerStore = useReaderStore()
const ragStore = useRagStore()

// Count of papers that have actually been opened (still present in the library).
const recentCount = computed(() => {
  void recentPapersVersion.value
  return filterRecentlyRead(library.papers).length
})

const showSettings = defineModel<boolean>('showSettings', { default: false })
const props = defineProps<{ snippetLibraryVisible?: boolean; activeSnippetLibraryId?: string | null }>()
const emit = defineEmits<{ 'open-canvas': [canvasId: string]; 'open-snippet-library': [libraryId: string] }>()

type SidebarMode = 'library' | 'canvas' | 'snippets'

const expanded = ref<Set<string>>(new Set())
const libraryCollapsed = ref(false)
const SIDEBAR_MODE_KEY = 'argus:sidebar:mode'
const EXPANDED_COLLECTIONS_KEY_PREFIX = 'argus:sidebar:expanded-collections'
const CANVAS_HEIGHT_KEY = 'argus:sidebar:canvas-height'
const canvasPanelHeight = ref(loadCanvasPanelHeight())
const canvasCollapsed = ref(false)
const showNewCanvasInput = ref(false)
const newCanvasName = ref('')
const TAGS_HEIGHT_KEY = 'argus:sidebar:tags-height'
const tagsPanelHeight = ref(loadTagsPanelHeight())
const sidebarMode = ref<SidebarMode>(loadSidebarMode())
const showWorkspaceMenu = ref(false)
const sidebarScrollRef = ref<HTMLElement | null>(null)
const collectionScrollRef = ref<HTMLElement | null>(null)

function loadSidebarMode(): SidebarMode {
  try {
    const raw = localStorage.getItem(SIDEBAR_MODE_KEY)
    if (raw === 'canvas' || raw === 'snippets' || raw === 'library') return raw
  } catch {}
  return 'library'
}

function saveSidebarMode(mode: SidebarMode) {
  try {
    localStorage.setItem(SIDEBAR_MODE_KEY, mode)
  } catch {}
}

function sidebarModeLabel(mode: SidebarMode): string {
  if (mode === 'canvas') return t('sidebar.canvas')
  if (mode === 'snippets') return t('snippets.title')
  return t('sidebar.library')
}

const activeSidebarModeLabel = computed(() => sidebarModeLabel(sidebarMode.value))

function setSidebarMode(mode: SidebarMode) {
  sidebarMode.value = mode
  saveSidebarMode(mode)
  showWorkspaceMenu.value = false
  closeCtx()
  if (mode === 'canvas') {
    void canvasStore.loadList()
  } else if (mode === 'snippets') {
    void reloadSnippets()
  }
}

function loadCanvasPanelHeight() {
  try {
    const raw = Number(localStorage.getItem(CANVAS_HEIGHT_KEY))
    if (Number.isFinite(raw) && raw > 0) return Math.min(400, Math.max(72, raw))
  } catch {}
  return 160
}
const EMOJI_PAGE_SIZE = 90

function loadTagsPanelHeight() {
  try {
    const raw = Number(localStorage.getItem(TAGS_HEIGHT_KEY))
    if (Number.isFinite(raw) && raw > 0) return Math.min(360, Math.max(56, raw))
  } catch {}
  return 128
}

function expandedCollectionsKey(path: string) {
  return `${EXPANDED_COLLECTIONS_KEY_PREFIX}:${encodeURIComponent(path)}`
}

function loadExpandedCollections(path: string) {
  try {
    const raw = localStorage.getItem(expandedCollectionsKey(path))
    if (!raw) return new Set<string>()
    const ids = JSON.parse(raw)
    if (!Array.isArray(ids)) return new Set<string>()
    return new Set(ids.filter((id): id is string => typeof id === 'string' && id.length > 0))
  } catch {
    return new Set<string>()
  }
}

function saveExpandedCollections() {
  if (!library.currentPath) return
  try {
    localStorage.setItem(
      expandedCollectionsKey(library.currentPath),
      JSON.stringify([...expanded.value])
    )
  } catch {}
}

onMounted(async () => {
  if (library.currentPath) {
    expanded.value = loadExpandedCollections(library.currentPath)
    await Promise.all([collectionsStore.load(), canvasStore.loadList()])
  }
  window.addEventListener('mousedown', onGlobalMousedown)
})

function onGlobalMousedown(e: MouseEvent) {
  const anyMenuOpen = showWorkspaceMenu.value || ctxMenu.value || canvasCtxMenu.value || libCtxMenu.value || snippetCtxMenu.value
  if (!anyMenuOpen) return
  const target = e.target as Element | null
  if (target?.closest('.workspace-switcher')) return
  if (target?.closest('.ctx-menu')) return
  closeCtx()
  showWorkspaceMenu.value = false
}

function select(item: NavItem) {
  selection.selectNav(item)
}

function toggleTag(tag: string) {
  selection.toggleTagFilter(tag)
}

const CATEGORY_ICONS: Record<string, string> = {
  '理论': '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/></svg>',
  '方法': '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>',
  '数据集': '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/></svg>',
}

const CATEGORY_STYLES: Record<string, { color: string; background: string }> = {
  '理论': { color: '#7c3aed', background: '#f3e8ff' },
  '方法': { color: '#2563eb', background: '#dbeafe' },
  '数据集': { color: '#059669', background: '#d1fae5' },
}

function categoryStyleFor(tag: string): { color: string; background: string } | undefined {
  return CATEGORY_STYLES[tag]
}

async function deleteTag(tag: string) {
  const confirmMsg = t('sidebar.deleteTagConfirm').replace('{name}', tag)
  if (!window.confirm(confirmMsg)) return
  try {
    await invoke('delete_tag', { tag })
    if (selection.tagFilter === tag) {
      selection.selectNav('all')
    }
    await library.refresh()
  } catch (e) {
    console.error('Failed to delete tag:', e)
  }
}

function toggleExpand(id: string) {
  if (expanded.value.has(id)) expanded.value.delete(id)
  else expanded.value.add(id)
  saveExpandedCollections()
}

watch(
  () => library.currentPath,
  (path) => {
    expanded.value = path ? loadExpandedCollections(path) : new Set()
  },
  { immediate: true }
)

function dataAttrSelectorValue(value: string) {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
}

async function scrollToCollection(collectionId: string | null) {
  await nextTick()
  window.requestAnimationFrame(() => {
    const scrollEl = collectionScrollRef.value ?? sidebarScrollRef.value
    if (!scrollEl) return
    if (!collectionId) {
      scrollEl.scrollTo({ top: 0, behavior: 'smooth' })
      return
    }
    const target = scrollEl.querySelector<HTMLElement>(
      `[data-collection-id="${dataAttrSelectorValue(collectionId)}"]`
    )
    target?.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'smooth' })
  })
}

async function syncSidebarToPaper(slug: string | null) {
  if (!slug) return
  const paper = library.papers.find(p => p.slug === slug)
  if (!paper) return

  if (sidebarMode.value !== 'library') {
    setSidebarMode('library')
  }
  libraryCollapsed.value = false

  const assignment = collectionsStore.file.assignments.find(a => a.paper_id === paper.id)
  if (!assignment) {
    selection.highlightCollection(null)
    await scrollToCollection(null)
    return
  }

  const collectionId = assignment.collection_id
  let cur = collectionsStore.collectionById(collectionId)
  const toExpand: string[] = []
  while (cur?.parent_id) {
    toExpand.push(cur.parent_id)
    cur = collectionsStore.collectionById(cur.parent_id)
  }
  toExpand.forEach(id => expanded.value.add(id))
  if (toExpand.length) saveExpandedCollections()

  selection.highlightCollection(collectionId)
  await scrollToCollection(collectionId)
}

// ── Sync sidebar position to selected/open paper ─────────────────────────────
watch(
  () => readerStore.activeSlug,
  (slug) => {
    void syncSidebarToPaper(slug)
  }
)

watch(
  () => selection.selectedSlug,
  (slug) => {
    void syncSidebarToPaper(slug)
  }
)

// ── New collection ─────────────────────────────────────────────────────────────
const refreshSpinning = ref(false)

async function handleLibraryRefresh() {
  if (refreshSpinning.value) return
  refreshSpinning.value = true
  const t0 = Date.now()
  try {
    await library.refresh()
  } finally {
    const remaining = 700 - (Date.now() - t0)
    if (remaining > 0) await new Promise(r => setTimeout(r, remaining))
    refreshSpinning.value = false
  }
}

const canvasRefreshSpinning = ref(false)

async function handleCanvasRefresh() {
  if (canvasRefreshSpinning.value) return
  canvasRefreshSpinning.value = true
  const t0 = Date.now()
  try {
    await canvasStore.loadList()
  } finally {
    const remaining = 700 - (Date.now() - t0)
    if (remaining > 0) await new Promise(r => setTimeout(r, remaining))
    canvasRefreshSpinning.value = false
  }
}

const snippetRefreshSpinning = ref(false)

async function handleSnippetRefresh() {
  if (snippetRefreshSpinning.value) return
  snippetRefreshSpinning.value = true
  const t0 = Date.now()
  try {
    await reloadSnippets()
  } finally {
    const remaining = 700 - (Date.now() - t0)
    if (remaining > 0) await new Promise(r => setTimeout(r, remaining))
    snippetRefreshSpinning.value = false
  }
}

const showNewInput = ref(false)
const newCollName = ref('')
const newCollParent = ref<string | undefined>(undefined)
let _newCollCompositionEndedAt = 0
function onNewCollCompositionEnd() { _newCollCompositionEndedAt = Date.now() }
function isNewCollIMEActive() { return Date.now() - _newCollCompositionEndedAt < 100 }

function startNew(parentId?: string) {
  newCollParent.value = parentId
  newCollName.value = ''
  showNewInput.value = true
  libraryCollapsed.value = false
  setTimeout(() => (document.getElementById('new-coll-input') as HTMLInputElement)?.focus(), 50)
}

async function submitNew() {
  const name = newCollName.value.trim()
  if (!name) { showNewInput.value = false; return }
  await collectionsStore.create(name, newCollParent.value)
  showNewInput.value = false
  newCollName.value = ''
}

// ── Rename ────────────────────────────────────────────────────────────────────
const renamingId = ref<string | null>(null)
const renameValue = ref('')

function startRename(col: Collection) {
  renamingId.value = col.id
  renameValue.value = col.name
  setTimeout(() => (document.getElementById(`rename-${col.id}`) as HTMLInputElement)?.focus(), 50)
}

async function submitRename(id: string) {
  const name = renameValue.value.trim()
  if (name) await collectionsStore.rename(id, name)
  renamingId.value = null
}

// ── Emoji picker (Fluent Emoji, full set) ────────────────────────────────────
const emojiPicker = ref<{ col: Collection } | null>(null)
const emojiDraft = ref('')
const emojiQuery = ref('')
const emojiPage = ref(0)

const filteredEmojis = computed<FluentEmojiEntry[]>(() => {
  const all = fluentEmojiList()
  const q = emojiQuery.value.trim().toLowerCase()
  if (!q) return all
  // Match the icon's descriptive name (e.g. "rocket", "red-heart") or the raw
  // character the user typed.
  const terms = q.split(/[\s-]+/).filter(Boolean)
  return all.filter(e => e.char === q || terms.every(term => e.name.includes(term)))
})
const emojiPageCount = computed(() => Math.max(1, Math.ceil(filteredEmojis.value.length / EMOJI_PAGE_SIZE)))
const visibleEmojis = computed(() => {
  const start = emojiPage.value * EMOJI_PAGE_SIZE
  return filteredEmojis.value.slice(start, start + EMOJI_PAGE_SIZE)
})
// Reset to the first page whenever the search changes.
watch(emojiQuery, () => { emojiPage.value = 0 })

function openEmojiPicker(col: Collection) {
  closeCtx()
  ensureFluentIcons()
  emojiPicker.value = { col }
  emojiDraft.value = col.emoji ?? ''
  emojiQuery.value = ''
  emojiPage.value = 0
  nextTick(() => (document.getElementById('collection-emoji-search') as HTMLInputElement | null)?.focus())
}

function closeEmojiPicker() {
  emojiPicker.value = null
  emojiDraft.value = ''
  emojiQuery.value = ''
  emojiPage.value = 0
}

function setEmojiPage(delta: number) {
  emojiPage.value = Math.max(0, Math.min(emojiPageCount.value - 1, emojiPage.value + delta))
}

async function saveCollectionEmoji(value = emojiDraft.value) {
  if (!emojiPicker.value) return
  const emoji = value.trim()
  if (!emoji) return
  await collectionsStore.setEmoji(emojiPicker.value.col.id, emoji)
  closeEmojiPicker()
}

// ── Delete ────────────────────────────────────────────────────────────────────
async function deleteCollection(col: Collection) {
  const confirmMsg = t('collections.deleteConfirm').replace('{name}', col.name)
  if (!window.confirm(confirmMsg)) return
  await collectionsStore.remove(col.id)
  if (selection.activeCollectionId === col.id) {
    selection.selectNav('all')
  }
}

// ── Open in Finder ────────────────────────────────────────────────────────────
async function openInFinder() {
  try {
    const path = await invoke<string>('get_papers_folder_path')
    await invoke('open_in_finder', { path })
  } catch (e) {
    console.error('Open in finder failed:', e)
  }
}

async function openCollectionInFinder(col: Collection) {
  try {
    const path = await invoke<string>('get_collection_folder_path', { collectionId: col.id })
    await invoke('open_in_finder', { path })
  } catch (e) {
    console.error('Open collection in finder failed:', e)
  }
}

// ── Canvas panel ──────────────────────────────────────────────────────────────
function startNewCanvas() {
  setSidebarMode('canvas')
  newCanvasName.value = t('canvas.newCanvasName')
  showNewCanvasInput.value = true
  canvasCollapsed.value = false
  setTimeout(() => (document.getElementById('new-canvas-input') as HTMLInputElement)?.focus(), 50)
}

async function submitNewCanvas() {
  const name = newCanvasName.value.trim()
  showNewCanvasInput.value = false
  if (!name) return
  try {
    await canvasStore.createCanvas(name)
    await openSpecificCanvas(canvasStore.canvasList[canvasStore.canvasList.length - 1]?.id)
  } catch (e) {
    console.error('create_canvas:', e)
  }
}

async function openSpecificCanvas(canvasId?: string) {
  if (!canvasId) return
  sidebarMode.value = 'canvas'
  saveSidebarMode('canvas')
  try {
    await canvasStore.openCanvas(canvasId)
    emit('open-canvas', canvasId)
  } catch (e) {
    console.error('Open canvas:', e)
  }
}

const canvasRenamingId = ref<string | null>(null)
const canvasRenameValue = ref('')

function startRenameCanvas(entry: CanvasIndexEntry) {
  closeCtx()
  canvasRenamingId.value = entry.id
  canvasRenameValue.value = entry.name
  nextTick(() => {
    const el = document.getElementById(`canvas-rename-${entry.id}`) as HTMLInputElement | null
    el?.focus()
    el?.select()
  })
}

function cancelRenameCanvas() {
  canvasRenamingId.value = null
  canvasRenameValue.value = ''
}

async function submitRenameCanvas(id: string) {
  const name = canvasRenameValue.value.trim()
  if (name) await canvasStore.renameCanvas(id, name)
  cancelRenameCanvas()
}

async function deleteCanvas(entry: CanvasIndexEntry) {
  const confirmMsg = t('canvas.deleteConfirm').replace('{name}', entry.name)
  if (!window.confirm(confirmMsg)) return
  await canvasStore.deleteCanvas(entry.id)
}

let canvasResizeStartY = 0
let canvasResizeStartH = 0
let isResizingCanvas = false

function clampCanvasHeight(h: number) {
  return Math.min(400, Math.max(72, h))
}

function startResizeCanvas(e: MouseEvent) {
  isResizingCanvas = true
  canvasResizeStartY = e.clientY
  canvasResizeStartH = canvasPanelHeight.value
  document.body.style.cursor = 'row-resize'
  document.body.style.userSelect = 'none'
  window.addEventListener('mousemove', onResizeCanvasMove)
  window.addEventListener('mouseup', stopResizeCanvas, { once: true })
}

function onResizeCanvasMove(e: MouseEvent) {
  if (!isResizingCanvas) return
  canvasPanelHeight.value = clampCanvasHeight(canvasResizeStartH + (canvasResizeStartY - e.clientY))
}

function stopResizeCanvas() {
  if (!isResizingCanvas) return
  isResizingCanvas = false
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  localStorage.setItem(CANVAS_HEIGHT_KEY, String(Math.round(canvasPanelHeight.value)))
  window.removeEventListener('mousemove', onResizeCanvasMove)
}

// ── Snippet Library panel ─────────────────────────────────────────────────────
const SNIPPET_HEIGHT_KEY = 'argus:sidebar:snippet-height'
const snippetPanelHeight = ref(loadSnippetPanelHeight())
const snippetCollapsed = ref(false)
const showNewSnippetLibInput = ref(false)
const newSnippetLibName = ref('')
const snippetRenamingId = ref<string | null>(null)
const snippetRenameValue = ref('')
const snippetCtxMenu = ref<{ x: number; y: number; id: string; name: string } | null>(null)
const activeSnippetLibraryId = ref<string | null>(null)
// Per-library embed progress, keyed by snippet library id (mirrors the
// collection embed badge). Only one snippet embed job runs at a time.
type SnippetEmbedJob = { done: number; total: number; failed: number; status: 'running' | 'done' }
const snippetEmbedJobs = ref<Record<string, SnippetEmbedJob>>({})

function loadSnippetPanelHeight() {
  try {
    const raw = Number(localStorage.getItem(SNIPPET_HEIGHT_KEY))
    if (Number.isFinite(raw) && raw > 0) return Math.min(400, Math.max(72, raw))
  } catch {}
  return 160
}

function startNewSnippetLib() {
  setSidebarMode('snippets')
  newSnippetLibName.value = ''
  showNewSnippetLibInput.value = true
  snippetCollapsed.value = false
  setTimeout(() => (document.getElementById('new-snippet-lib-input') as HTMLInputElement)?.focus(), 50)
}

async function submitNewSnippetLib() {
  const name = newSnippetLibName.value.trim()
  showNewSnippetLibInput.value = false
  if (!name) return
  const lib = await createSnippetLibrary(name)
  openSnippetLibrary(lib.id)
}

function openSnippetLibrary(id: string) {
  sidebarMode.value = 'snippets'
  saveSidebarMode('snippets')
  activeSnippetLibraryId.value = id
  emit('open-snippet-library', id)
}

function startRenameSnippetLib(id: string, name: string) {
  closeSnippetCtx()
  snippetRenamingId.value = id
  snippetRenameValue.value = name
  nextTick(() => {
    const el = document.getElementById(`snippet-lib-rename-${id}`) as HTMLInputElement | null
    el?.focus()
    el?.select()
  })
}

async function submitRenameSnippetLib(id: string) {
  const name = snippetRenameValue.value.trim()
  if (name) await renameSnippetLibrary(id, name)
  snippetRenamingId.value = null
  snippetRenameValue.value = ''
}

async function handleDeleteSnippetLib(id: string, name: string) {
  closeSnippetCtx()
  if (!window.confirm(`删除素材库"${name}"及其所有素材？`)) return
  await deleteSnippetLibrary(id)
  if (activeSnippetLibraryId.value === id) activeSnippetLibraryId.value = null
}

function openSnippetCtx(e: MouseEvent, id: string, name: string) {
  e.preventDefault()
  ctxMenu.value = null
  libCtxMenu.value = null
  canvasCtxMenu.value = null
  snippetCtxMenu.value = { x: e.clientX, y: e.clientY, id, name }
}

function closeSnippetCtx() {
  snippetCtxMenu.value = null
}

async function openSnippetLibInFinder() {
  try {
    const path = await invoke<string>('get_snippets_folder_path')
    await invoke('open_in_finder', { path })
  } catch (e) {
    console.error('Open snippets in finder failed:', e)
  }
}

function removeSnippetEmbedJob(id: string) {
  const { [id]: _removed, ...rest } = snippetEmbedJobs.value
  snippetEmbedJobs.value = rest
}

// Embed this library's not-yet-vectorized snippets. Progress shows in the
// library row's badge, driven by the store-wide `snippet-embed-progress` event
// (the backend embeds one library at a time, so a single job is unambiguous).
async function buildSnippetEmbeddings(id: string) {
  if (snippetEmbedJobs.value[id]?.status === 'running' || !ragStore.isConfigured) return
  snippetEmbedJobs.value = { ...snippetEmbedJobs.value, [id]: { done: 0, total: 0, failed: 0, status: 'running' } }
  let unlisten: UnlistenFn | null = null
  try {
    unlisten = await listen<{ done: number; failed: number; total: number }>('snippet-embed-progress', (ev) => {
      const prev = snippetEmbedJobs.value[id]
      if (!prev || prev.status !== 'running') return
      snippetEmbedJobs.value = {
        ...snippetEmbedJobs.value,
        [id]: { ...prev, done: ev.payload.done, total: ev.payload.total, failed: ev.payload.failed },
      }
    })
    const [done, failed] = await invoke<[number, number]>('embed_library_snippets', { libraryId: id })
    snippetEmbedJobs.value = { ...snippetEmbedJobs.value, [id]: { done, total: done + failed, failed, status: 'done' } }
    setTimeout(() => removeSnippetEmbedJob(id), failed > 0 ? 5000 : 2500)
  } catch {
    removeSnippetEmbedJob(id)
  } finally {
    unlisten?.()
  }
}

function snippetCountFor(id: string) {
  return allSnippets.value.filter(s => s.libraryId === id).length
}

let snippetResizeStartY = 0
let snippetResizeStartH = 0
let isResizingSnippet = false

function startResizeSnippet(e: MouseEvent) {
  isResizingSnippet = true
  snippetResizeStartY = e.clientY
  snippetResizeStartH = snippetPanelHeight.value
  document.body.style.cursor = 'row-resize'
  document.body.style.userSelect = 'none'
  window.addEventListener('mousemove', onResizeSnippetMove)
  window.addEventListener('mouseup', stopResizeSnippet, { once: true })
}

function onResizeSnippetMove(e: MouseEvent) {
  if (!isResizingSnippet) return
  snippetPanelHeight.value = Math.min(400, Math.max(72, snippetResizeStartH + (snippetResizeStartY - e.clientY)))
}

function stopResizeSnippet() {
  if (!isResizingSnippet) return
  isResizingSnippet = false
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  localStorage.setItem(SNIPPET_HEIGHT_KEY, String(Math.round(snippetPanelHeight.value)))
  window.removeEventListener('mousemove', onResizeSnippetMove)
}

// ── Context menu (collections) ────────────────────────────────────────────────
const ctxMenu = ref<{ x: number; y: number; col: Collection } | null>(null)
const canvasCtxMenu = ref<{ x: number; y: number; entry: CanvasIndexEntry } | null>(null)

function openCtx(e: MouseEvent, col: Collection) {
  e.preventDefault()
  libCtxMenu.value = null
  canvasCtxMenu.value = null
  snippetCtxMenu.value = null
  ctxMenu.value = { x: e.clientX, y: e.clientY, col }
}

function openCanvasCtx(e: MouseEvent, entry: CanvasIndexEntry) {
  e.preventDefault()
  ctxMenu.value = null
  libCtxMenu.value = null
  snippetCtxMenu.value = null
  canvasCtxMenu.value = { x: e.clientX, y: e.clientY, entry }
}

async function openCanvasesInFinder() {
  try {
    const path = await invoke<string>('get_canvases_folder_path')
    await invoke('open_in_finder', { path })
  } catch (e) {
    console.error('Open canvases in finder failed:', e)
  }
}

// Embed all not-yet-vectorized papers in the collection tree (progress shows
// in the collection row's badge, driven by ragStore.collectionEmbedJobs).
function embedCollectionTree(col: Collection) {
  const papers = collectionsStore.listAllPapersInTree(col.id)
  ragStore.embedCollection(col.id, papers)
}

// ── Context menu (All Papers / library root) ──────────────────────────────────
const libCtxMenu = ref<{ x: number; y: number } | null>(null)

function openLibCtx(e: MouseEvent) {
  e.preventDefault()
  ctxMenu.value = null
  canvasCtxMenu.value = null
  snippetCtxMenu.value = null
  libCtxMenu.value = { x: e.clientX, y: e.clientY }
}

function closeCtx() {
  ctxMenu.value = null
  libCtxMenu.value = null
  canvasCtxMenu.value = null
  snippetCtxMenu.value = null
}

// ── Drag-drop targets (driven by pointer-based drag in PaperList) ─────────────
const dragOverId = ref<string | null>(null)
const collectionDraggingId = ref<string | null>(null)
const collectionDragOverId = ref<string | null>(null)
const collectionRootDragOver = ref(false)
const collectionDragGhost = ref<Collection | null>(null)
const collectionDragGhostPos = ref({ x: 0, y: 0 })
let collectionDragCleanup: (() => void) | null = null

function onPaperDragOver(e: Event) {
  dragOverId.value = (e as CustomEvent<{ collectionId: string | null }>).detail.collectionId
}

const snippetDragOverLibraryId = ref<string | null>(null)

function onSnippetDragOver(e: Event) {
  snippetDragOverLibraryId.value = (e as CustomEvent<{ libraryId: string | null }>).detail.libraryId
}

onMounted(() => {
  document.addEventListener('argus-paper-drag-over', onPaperDragOver)
  document.addEventListener('argus-snippet-drag-over', onSnippetDragOver)
})

function isCollectionDescendant(collectionId: string, ancestorId: string) {
  let cur = collectionsStore.collectionById(collectionId)
  const visited = new Set<string>()
  while (cur?.parent_id) {
    if (cur.parent_id === ancestorId) return true
    if (visited.has(cur.parent_id)) return false
    visited.add(cur.parent_id)
    cur = collectionsStore.collectionById(cur.parent_id)
  }
  return false
}

function findCollectionDropTarget(x: number, y: number): string | null | undefined {
  for (const el of document.elementsFromPoint(x, y)) {
    const dataset = (el as HTMLElement).dataset
    if (dataset?.collectionId) return dataset.collectionId
    if (dataset?.collectionRoot === 'true') return null
  }
  return undefined
}

function validCollectionDropTarget(source: Collection, rawTarget: string | null | undefined) {
  if (rawTarget === undefined) return undefined
  if (rawTarget === source.id) return undefined
  if (rawTarget && isCollectionDescendant(rawTarget, source.id)) return undefined
  if ((source.parent_id ?? null) === (rawTarget ?? null)) return undefined
  return rawTarget ?? null
}

function setCollectionDropHover(target: string | null | undefined) {
  collectionDragOverId.value = typeof target === 'string' ? target : null
  collectionRootDragOver.value = target === null
}

function clearCollectionDragState() {
  collectionDraggingId.value = null
  collectionDragOverId.value = null
  collectionRootDragOver.value = false
  collectionDragGhost.value = null
  document.body.style.cursor = ''
}

function startCollectionDrag(e: MouseEvent, col: Collection) {
  if (e.button !== 0 || renamingId.value === col.id) return
  collectionDragCleanup?.()
  const startX = e.clientX
  const startY = e.clientY
  let dragging = false

  function onMove(ev: MouseEvent) {
    if (!dragging && Math.hypot(ev.clientX - startX, ev.clientY - startY) < 6) return
    if (!dragging) {
      dragging = true
      closeCtx()
      collectionDraggingId.value = col.id
      collectionDragGhost.value = col
      document.body.style.cursor = 'grabbing'
    }

    ev.preventDefault()
    collectionDragGhostPos.value = { x: ev.clientX + 14, y: ev.clientY + 10 }
    const rawTarget = findCollectionDropTarget(ev.clientX, ev.clientY)
    setCollectionDropHover(validCollectionDropTarget(col, rawTarget))
  }

  async function onUp(ev: MouseEvent) {
    collectionDragCleanup?.()
    collectionDragCleanup = null

    if (!dragging) {
      clearCollectionDragState()
      return
    }

    const rawTarget = findCollectionDropTarget(ev.clientX, ev.clientY)
    const target = validCollectionDropTarget(col, rawTarget)
    clearCollectionDragState()
    if (target === undefined) return

    try {
      await collectionsStore.moveCollection(col.id, target)
      if (target) {
        expanded.value.add(target)
        saveExpandedCollections()
      }
      await library.refresh()
    } catch (err) {
      console.error('move_collection:', err)
    }
  }

  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
  collectionDragCleanup = () => {
    window.removeEventListener('mousemove', onMove)
    window.removeEventListener('mouseup', onUp)
  }
}

// ── Tags panel resize ─────────────────────────────────────────────────────────
let tagResizeStartY = 0
let tagResizeStartH = 0
let isResizingTags = false

function clampTagsHeight(height: number) {
  const max = Math.min(360, Math.max(120, window.innerHeight * 0.48))
  return Math.min(max, Math.max(56, height))
}

function startResizeTags(e: MouseEvent) {
  isResizingTags = true
  tagResizeStartY = e.clientY
  tagResizeStartH = tagsPanelHeight.value
  document.body.style.cursor = 'row-resize'
  document.body.style.userSelect = 'none'
  window.addEventListener('mousemove', onResizeTagsMove)
  window.addEventListener('mouseup', stopResizeTags, { once: true })
}

function onResizeTagsMove(e: MouseEvent) {
  if (!isResizingTags) return
  tagsPanelHeight.value = clampTagsHeight(tagResizeStartH + (tagResizeStartY - e.clientY))
}

function stopResizeTags() {
  if (!isResizingTags) return
  isResizingTags = false
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
  localStorage.setItem(TAGS_HEIGHT_KEY, String(Math.round(tagsPanelHeight.value)))
  window.removeEventListener('mousemove', onResizeTagsMove)
}

onUnmounted(() => {
  collectionDragCleanup?.()
  collectionDragCleanup = null
  clearCollectionDragState()
  document.removeEventListener('argus-paper-drag-over', onPaperDragOver)
  document.removeEventListener('argus-snippet-drag-over', onSnippetDragOver)
  window.removeEventListener('mousemove', onResizeTagsMove)
  window.removeEventListener('mouseup', stopResizeTags)
  window.removeEventListener('mousemove', onResizeCanvasMove)
  window.removeEventListener('mouseup', stopResizeCanvas)
  window.removeEventListener('mousedown', onGlobalMousedown)
})
</script>

<template>
  <div class="left-sidebar" @click="closeCtx">
    <div
      ref="sidebarScrollRef"
      class="sidebar-scroll"
      :class="{ 'collection-root-drop-zone': sidebarMode === 'library' && collectionRootDragOver }"
      :data-collection-root="sidebarMode === 'library' ? 'true' : undefined"
    >
      <div class="workspace-header">
        <div class="workspace-switcher">
          <button class="workspace-switcher-btn" @click.stop="showWorkspaceMenu = !showWorkspaceMenu">
            <span class="workspace-icon" aria-hidden="true">
              <svg v-if="sidebarMode === 'library'" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
                <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
              </svg>
              <svg v-else-if="sidebarMode === 'canvas'" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="8" cy="8" r="2"/><circle cx="16" cy="8" r="2"/><circle cx="12" cy="17" r="2"/>
                <line x1="9.8" y1="8.8" x2="13.2" y2="15.4"/><line x1="14.2" y1="8.8" x2="10.8" y2="15.4"/><line x1="10" y1="8" x2="14" y2="8"/>
              </svg>
              <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
              </svg>
            </span>
            <span class="workspace-title">{{ activeSidebarModeLabel }}</span>
            <svg
              width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
              class="workspace-caret"
              :class="{ 'is-open': showWorkspaceMenu }"
            >
              <polyline points="6 9 12 15 18 9"/>
            </svg>
          </button>

          <Transition name="workspace-menu">
            <div v-if="showWorkspaceMenu" class="workspace-menu" @click.stop>
              <button
                class="workspace-menu-item"
                :class="{ active: sidebarMode === 'library' }"
                @click="setSidebarMode('library')"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
                  <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
                </svg>
                {{ t('sidebar.library') }}
              </button>
              <button
                class="workspace-menu-item"
                :class="{ active: sidebarMode === 'canvas' }"
                @click="setSidebarMode('canvas')"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <circle cx="8" cy="8" r="2"/><circle cx="16" cy="8" r="2"/><circle cx="12" cy="17" r="2"/>
                  <line x1="9.8" y1="8.8" x2="13.2" y2="15.4"/><line x1="14.2" y1="8.8" x2="10.8" y2="15.4"/><line x1="10" y1="8" x2="14" y2="8"/>
                </svg>
                {{ t('sidebar.canvas') }}
              </button>
              <button
                class="workspace-menu-item"
                :class="{ active: sidebarMode === 'snippets' }"
                @click="setSidebarMode('snippets')"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
                </svg>
                {{ t('snippets.title') }}
              </button>
            </div>
          </Transition>
        </div>

        <div class="section-header-right">
          <button
            v-if="sidebarMode === 'library'"
            class="icon-action"
            :title="t('toolbar.refreshTitle')"
            :disabled="refreshSpinning"
            @click.stop="handleLibraryRefresh()"
          >
            <svg
              width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              :class="{ spin: refreshSpinning }"
            >
              <polyline points="23 4 23 10 17 10"/>
              <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
            </svg>
          </button>
          <button
            v-else-if="sidebarMode === 'canvas'"
            class="icon-action"
            :title="t('toolbar.refreshTitle')"
            :disabled="canvasRefreshSpinning"
            @click.stop="handleCanvasRefresh()"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              :class="{ spin: canvasRefreshSpinning }">
              <polyline points="23 4 23 10 17 10"/>
              <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
            </svg>
          </button>
          <button
            v-else
            class="icon-action"
            :title="t('toolbar.refreshTitle')"
            :disabled="snippetRefreshSpinning"
            @click.stop="handleSnippetRefresh()"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              :class="{ spin: snippetRefreshSpinning }">
              <polyline points="23 4 23 10 17 10"/>
              <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
            </svg>
          </button>

          <button
            v-if="sidebarMode === 'library'"
            class="icon-action"
            :title="t('collections.new')"
            @click.stop="startNew()"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/>
              <line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
          </button>
          <button
            v-else-if="sidebarMode === 'canvas'"
            class="icon-action"
            :title="t('canvas.newCanvas')"
            @click.stop="startNewCanvas()"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/>
              <line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
          </button>
          <button
            v-else
            class="icon-action"
            :title="t('snippets.newLibrary')"
            @click.stop="startNewSnippetLib()"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/>
              <line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
          </button>
        </div>
      </div>

      <template v-if="sidebarMode === 'library'">
        <div class="library-content">
          <div class="library-pinned-nav">
            <div
              class="all-papers-section"
              :class="{ 'collection-root-drop-over': collectionRootDragOver }"
              data-collection-root="true"
            >
              <button
              class="nav-item"
              :class="{ active: selection.activeNav === 'all' && !selection.highlightedCollectionId }"
                @click="select('all')"
                @contextmenu.prevent="openLibCtx($event)"
              >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <rect x="3" y="3" width="7" height="7"/>
                  <rect x="14" y="3" width="7" height="7"/>
                  <rect x="14" y="14" width="7" height="7"/>
                  <rect x="3" y="14" width="7" height="7"/>
                </svg>
                {{ t('sidebar.allPapers') }}
                <span class="badge">{{ library.papers.length }}</span>
              </button>
            </div>

            <div class="all-papers-section">
              <button
              class="nav-item"
              :class="{ active: selection.activeNav === 'recent' && !selection.highlightedCollectionId }"
                @click="select('recent')"
              >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="12" r="9"/>
                  <polyline points="12 7 12 12 15 14"/>
                </svg>
                {{ t('sidebar.recentPapers') }}
                <span class="badge">{{ recentCount }}</span>
              </button>
            </div>
          </div>

          <div
            ref="collectionScrollRef"
            class="library-main-scroll"
            :class="{ 'collection-root-drop-over': collectionRootDragOver }"
            data-collection-root="true"
          >
            <div
              class="coll-list"
              :class="{ 'collection-root-drop-over': collectionRootDragOver }"
              data-collection-root="true"
            >
              <div v-if="showNewInput && !newCollParent" class="new-coll-row">
                <input
                  id="new-coll-input"
                  v-model="newCollName"
                  class="coll-name-input"
                  :placeholder="t('collections.namePlaceholder')"
                  @compositionend="onNewCollCompositionEnd"
                  @keydown.enter="() => { if (!isNewCollIMEActive()) submitNew() }"
                  @keydown.escape="showNewInput = false"
                  @blur="submitNew"
                />
              </div>

              <div v-if="collectionsStore.topLevel.length === 0 && !showNewInput" class="no-collections">
                {{ t('collections.noCollections') }}
              </div>

              <CollectionNode
                v-for="col in collectionsStore.topLevel"
                :key="col.id"
                :collection="col"
                :depth="0"
                :expanded="expanded"
                :renaming-id="renamingId"
                :rename-value="renameValue"
                :drag-over-id="dragOverId"
                :collection-drag-over-id="collectionDragOverId"
                :collection-dragging-id="collectionDraggingId"
                :show-new-input="showNewInput"
                :new-coll-parent="newCollParent"
                :new-coll-name="newCollName"
                @toggle-expand="toggleExpand"
                @open-ctx="openCtx"
                @start-rename="startRename"
                @submit-rename="submitRename"
                @delete="deleteCollection"
                @start-new="startNew"
                @collection-drag-start="startCollectionDrag"
                @submit-new="submitNew"
                @update:renameValue="renameValue = $event"
                @update:newCollName="newCollName = $event"
                @update:showNewInput="showNewInput = $event"
              />
            </div>
          </div>

          <div
            v-if="library.allTags.length > 0"
            class="tags-panel tags-panel-fixed"
            :style="{ maxHeight: `${tagsPanelHeight}px` }"
          >
            <div class="tags-resize-handle" @mousedown.prevent.stop="startResizeTags" />
            <div class="section-title tags-title">{{ t('sidebar.tags') }}</div>
            <div class="tag-cloud">
              <div
                v-for="tag in library.allTags"
                :key="tag"
                class="tag-chip"
                :class="{ active: selection.tagFilter === tag }"
                :style="selection.tagFilter === tag ? {} : categoryStyleFor(tag)"
                @click="toggleTag(tag)"
              >
                <span
                  v-if="CATEGORY_ICONS[tag]"
                  class="tag-chip-icon"
                  v-html="CATEGORY_ICONS[tag]"
                />
                <span class="tag-chip-text">{{ tag }}</span>
                <button
                  class="tag-chip-delete"
                  :title="t('common.delete')"
                  @click.stop="deleteTag(tag)"
                >
                  <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
                    <path d="M18 6L6 18M6 6l12 12" />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>
      </template>

      <template v-else-if="sidebarMode === 'canvas'">
        <div class="workspace-panel">
          <div class="canvas-list mode-list">
          <div v-if="showNewCanvasInput" class="new-coll-row">
            <input
              id="new-canvas-input"
              v-model="newCanvasName"
              class="coll-name-input"
              :placeholder="t('canvas.namePlaceholder')"
              @keydown.enter="submitNewCanvas"
              @keydown.escape="showNewCanvasInput = false"
              @blur="submitNewCanvas"
            />
          </div>

          <div v-if="canvasStore.canvasList.length === 0 && !showNewCanvasInput" class="no-collections">
            {{ t('canvas.noCanvases') }}
          </div>

          <div
            v-for="cv in canvasStore.canvasList"
            :key="cv.id"
            class="nav-item"
            :class="{ active: canvasStore.isShown && canvasStore.currentCanvas?.id === cv.id }"
            role="button"
            tabindex="0"
            @click="openSpecificCanvas(cv.id)"
            @keydown.enter.prevent="openSpecificCanvas(cv.id)"
            @contextmenu.prevent.stop="openCanvasCtx($event, cv)"
          >
            <template v-if="canvasRenamingId === cv.id">
              <input
                :id="`canvas-rename-${cv.id}`"
                v-model="canvasRenameValue"
                class="coll-name-input canvas-rename-input"
                @click.stop
                @keydown.enter.stop.prevent="submitRenameCanvas(cv.id)"
                @keydown.escape.stop.prevent="cancelRenameCanvas"
                @blur="submitRenameCanvas(cv.id)"
              />
            </template>
            <template v-else>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="8" cy="8" r="2"/><circle cx="16" cy="8" r="2"/>
                <circle cx="12" cy="17" r="2"/>
                <line x1="9.8" y1="8.8" x2="13.2" y2="15.4"/>
                <line x1="14.2" y1="8.8" x2="10.8" y2="15.4"/>
                <line x1="10" y1="8" x2="14" y2="8"/>
              </svg>
              <span class="canvas-name-text">{{ cv.name }}</span>
              <span v-if="cv.node_count > 0" class="badge">{{ cv.node_count }}</span>
            </template>
          </div>
        </div>
        </div>
      </template>

      <template v-else>
        <div class="workspace-panel">
          <div class="canvas-list mode-list">
          <div v-if="showNewSnippetLibInput" class="new-coll-row">
            <input
              id="new-snippet-lib-input"
              v-model="newSnippetLibName"
              class="coll-name-input"
              :placeholder="t('snippets.libraryName')"
              @keydown.enter="submitNewSnippetLib"
              @keydown.escape="showNewSnippetLibInput = false"
              @blur="submitNewSnippetLib"
            />
          </div>

          <div v-if="snippetLibraries.length === 0 && !showNewSnippetLibInput" class="no-collections">
            {{ t('snippets.noLibraries') }}
          </div>

          <div
            v-for="lib in snippetLibraries"
            :key="lib.id"
            class="nav-item"
            :class="{
              active: props.snippetLibraryVisible && (props.activeSnippetLibraryId ?? activeSnippetLibraryId) === lib.id,
              'drag-over': snippetDragOverLibraryId === lib.id,
            }"
            :data-snippet-library-id="lib.id"
            role="button"
            tabindex="0"
            @click="openSnippetLibrary(lib.id)"
            @keydown.enter.prevent="openSnippetLibrary(lib.id)"
            @contextmenu.prevent.stop="openSnippetCtx($event, lib.id, lib.name)"
          >
            <template v-if="snippetRenamingId === lib.id">
              <input
                :id="`snippet-lib-rename-${lib.id}`"
                v-model="snippetRenameValue"
                class="coll-name-input canvas-rename-input"
                @click.stop
                @keydown.enter.stop.prevent="submitRenameSnippetLib(lib.id)"
                @keydown.escape.stop.prevent="snippetRenamingId = null"
                @blur="submitRenameSnippetLib(lib.id)"
              />
            </template>
            <template v-else>
              <span v-if="lib.emoji" style="font-size:12px">{{ lib.emoji }}</span>
              <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
              </svg>
              <span class="canvas-name-text">{{ lib.name }}</span>
              <span
                v-if="snippetEmbedJobs[lib.id]"
                class="badge embed-badge"
                :class="{ done: snippetEmbedJobs[lib.id].status === 'done' }"
              >
                <template v-if="snippetEmbedJobs[lib.id].status === 'running'">{{ snippetEmbedJobs[lib.id].total > 0 ? `${snippetEmbedJobs[lib.id].done + snippetEmbedJobs[lib.id].failed}/${snippetEmbedJobs[lib.id].total}` : '…' }}</template>
                <template v-else-if="snippetEmbedJobs[lib.id].failed > 0">{{ t('collections.embedFailedCount', { n: snippetEmbedJobs[lib.id].failed }) }}</template>
                <template v-else>✓</template>
              </span>
              <span v-else-if="snippetCountFor(lib.id) > 0" class="badge">{{ snippetCountFor(lib.id) }}</span>
            </template>
          </div>
        </div>
        </div>
      </template>
    </div>

    <div class="sidebar-footer">
      <button class="settings-nav-btn" @click.stop="showSettings = true">
        <span class="settings-icon-wrap">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
          </svg>
          <span v-if="updateStore.hasUpdate" class="update-dot" title="有新版本可用" />
        </span>
        <span>{{ t('settings.title') }}</span>
      </button>
    </div>

    <Teleport to="body">
      <div
        v-if="collectionDragGhost"
        class="collection-drag-ghost"
        :style="{ left: `${collectionDragGhostPos.x}px`, top: `${collectionDragGhostPos.y}px` }"
      >
        <span>{{ collectionDragGhost.emoji?.trim() || '📚' }}</span>
        <span>{{ collectionDragGhost.name }}</span>
      </div>
    </Teleport>

    <!-- Context menu (collection) -->
    <Teleport to="body">
      <div
        v-if="ctxMenu"
        class="ctx-menu"
        :style="{ left: ctxMenu.x + 'px', top: ctxMenu.y + 'px' }"
        @click.stop
      >
        <button class="ctx-item" @click="startRename(ctxMenu!.col); closeCtx()">
          {{ t('collections.rename') }}
        </button>
        <button
          v-if="!ctxMenu!.col.parent_id"
          class="ctx-item"
          @click="openEmojiPicker(ctxMenu!.col)"
        >
          {{ t('collections.setEmoji') }}
        </button>
        <button class="ctx-item" @click="startNew(ctxMenu!.col.id); closeCtx()">
          {{ t('collections.newSub') }}
        </button>
        <button class="ctx-item" @click="openCollectionInFinder(ctxMenu!.col); closeCtx()">
          {{ t('collections.openInFinder') }}
        </button>
        <template v-if="ragStore.isConfigured">
          <div class="ctx-sep" />
          <button
            class="ctx-item"
            :disabled="!!ragStore.collectionEmbedJobs[ctxMenu!.col.id]"
            @click="embedCollectionTree(ctxMenu!.col); closeCtx()"
          >
            {{ ragStore.collectionEmbedJobs[ctxMenu!.col.id] ? t('collections.buildEmbeddingsRunning') : t('collections.buildEmbeddings') }}
          </button>
        </template>
        <div class="ctx-sep" />
        <button class="ctx-item danger" @click="deleteCollection(ctxMenu!.col); closeCtx()">
          {{ t('collections.delete') }}
        </button>
      </div>
    </Teleport>

    <!-- Context menu (canvas item) -->
    <Teleport to="body">
      <div
        v-if="canvasCtxMenu"
        class="ctx-menu"
        :style="{ left: canvasCtxMenu.x + 'px', top: canvasCtxMenu.y + 'px' }"
        @click.stop
      >
        <button class="ctx-item" @click="startRenameCanvas(canvasCtxMenu!.entry)">
          {{ t('canvas.rename') }}
        </button>
        <button class="ctx-item" @click="openCanvasesInFinder(); closeCtx()">
          {{ t('collections.openInFinder') }}
        </button>
        <div class="ctx-sep" />
        <button class="ctx-item danger" @click="deleteCanvas(canvasCtxMenu!.entry); closeCtx()">
          {{ t('canvas.delete') }}
        </button>
      </div>
    </Teleport>

    <!-- Emoji picker -->
    <Teleport to="body">
      <div
        v-if="emojiPicker"
        class="emoji-picker-backdrop"
        @mousedown.self="closeEmojiPicker"
      >
        <div class="emoji-picker-modal" @mousedown.stop>
          <div class="emoji-picker-header">
            <div>
              <div class="emoji-picker-title">{{ t('collections.setEmoji') }}</div>
              <div class="emoji-picker-subtitle">{{ emojiPicker.col.name }}</div>
            </div>
            <button class="emoji-picker-close" @click="closeEmojiPicker">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4">
                <line x1="18" y1="6" x2="6" y2="18"/>
                <line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>

          <input
            id="collection-emoji-search"
            v-model="emojiQuery"
            class="emoji-search"
            :placeholder="t('collections.emojiSearchPlaceholder')"
            @keydown.escape.prevent="closeEmojiPicker"
          />

          <div class="emoji-grid" role="listbox">
            <button
              v-for="entry in visibleEmojis"
              :key="entry.name"
              class="emoji-option"
              :class="{ active: entry.char === emojiDraft }"
              :title="entry.name"
              @click="saveCollectionEmoji(entry.char)"
            >
              <Icon v-if="fluentReady" :icon="`fluent-emoji-flat:${entry.name}`" width="24" height="24" />
              <template v-else>{{ entry.char }}</template>
            </button>
            <div v-if="!visibleEmojis.length" class="emoji-empty">{{ t('collections.emojiNoMatch') }}</div>
          </div>

          <div class="emoji-pager">
            <button class="emoji-pager-btn" :disabled="emojiPage === 0" @click="setEmojiPage(-1)">
              {{ t('collections.emojiPrev') }}
            </button>
            <span>{{ emojiPage + 1 }} / {{ emojiPageCount }}</span>
            <button class="emoji-pager-btn" :disabled="emojiPage >= emojiPageCount - 1" @click="setEmojiPage(1)">
              {{ t('collections.emojiNext') }}
            </button>
          </div>

          <div class="emoji-manual">
            <input
              id="collection-emoji-input"
              v-model="emojiDraft"
              class="emoji-input"
              :placeholder="t('collections.emojiInputPlaceholder')"
              @keydown.enter.prevent="saveCollectionEmoji()"
              @keydown.escape.prevent="closeEmojiPicker"
            />
            <button
              class="emoji-save-btn"
              :disabled="!emojiDraft.trim()"
              @click="saveCollectionEmoji()"
            >
              {{ t('metaEdit.save') }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- Context menu (All Papers / library) -->
    <Teleport to="body">
      <div
        v-if="libCtxMenu"
        class="ctx-menu"
        :style="{ left: libCtxMenu.x + 'px', top: libCtxMenu.y + 'px' }"
        @click.stop
      >
        <button class="ctx-item" @click="startNew(); closeCtx()">
          {{ t('collections.new') }}
        </button>
        <button class="ctx-item" @click="openInFinder(); closeCtx()">
          {{ t('collections.openInFinder') }}
        </button>
      </div>
    </Teleport>

    <Teleport to="body">
      <div
        v-if="snippetCtxMenu"
        class="ctx-menu"
        :style="{ left: snippetCtxMenu.x + 'px', top: snippetCtxMenu.y + 'px' }"
        @click.stop
      >
        <button class="ctx-item" @click="startRenameSnippetLib(snippetCtxMenu!.id, snippetCtxMenu!.name); closeSnippetCtx()">
          重命名
        </button>
        <button class="ctx-item" @click="openSnippetLibInFinder(); closeSnippetCtx()">
          {{ t('collections.openInFinder') }}
        </button>
        <template v-if="ragStore.isConfigured">
          <div class="ctx-sep" />
          <button
            class="ctx-item"
            :disabled="snippetEmbedJobs[snippetCtxMenu!.id]?.status === 'running'"
            @click="buildSnippetEmbeddings(snippetCtxMenu!.id); closeSnippetCtx()"
          >
            {{ snippetEmbedJobs[snippetCtxMenu!.id]?.status === 'running' ? t('collections.buildEmbeddingsRunning') : t('collections.buildEmbeddings') }}
          </button>
        </template>
        <div class="ctx-sep" />
        <button class="ctx-item danger" @click="handleDeleteSnippetLib(snippetCtxMenu!.id, snippetCtxMenu!.name)">
          删除
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.left-sidebar {
  display: flex;
  flex-direction: column;
  background: var(--bg-secondary);
  border-right: 1px solid var(--border-subtle);
  overflow: hidden;
  padding: 0;
  user-select: none;
}

.sidebar-scroll {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 0;
}
.sidebar-scroll.collection-root-drop-zone {
  background: linear-gradient(
    to bottom,
    transparent 0,
    transparent 46px,
    color-mix(in srgb, var(--accent) 4%, transparent) 46px,
    color-mix(in srgb, var(--accent) 4%, transparent) 100%
  );
}

.workspace-header {
  flex-shrink: 0;
  position: relative;
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  height: var(--content-header-height);
  min-height: var(--content-header-height);
  padding: 0 8px 0 10px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}

.workspace-switcher {
  position: relative;
  flex: 1;
  min-width: 0;
}

.workspace-switcher-btn {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  max-width: 100%;
  min-width: 0;
  height: 28px;
  padding: 0 7px 0 6px;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  transition: background 0.12s, color 0.12s;
}
.workspace-switcher-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.workspace-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--text-tertiary);
}

.workspace-title {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 14px;
  font-weight: 650;
  color: var(--text-primary);
}

.workspace-caret {
  flex-shrink: 0;
  color: var(--text-tertiary);
  transition: transform 0.16s ease;
}
.workspace-caret.is-open { transform: rotate(180deg); }

.workspace-menu {
  position: absolute;
  top: calc(100% + 6px);
  left: 0;
  width: min(218px, calc(100vw - 24px));
  padding: 5px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  box-shadow: var(--shadow-md);
  z-index: 50;
}

.workspace-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-height: 30px;
  padding: 5px 8px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  font-size: 13px;
  text-align: left;
}
.workspace-menu-item:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.workspace-menu-item.active {
  background: var(--bg-active);
  color: var(--accent);
  font-weight: 600;
}
.workspace-menu-item svg { flex-shrink: 0; }

.workspace-menu-enter-active,
.workspace-menu-leave-active {
  transition: opacity 0.12s ease, transform 0.12s ease;
}
.workspace-menu-enter-from,
.workspace-menu-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

.workspace-panel {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 6px 0 10px;
}

.section { margin-bottom: 4px; }

/* Collapsible section header */
.section-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 28px;
  padding: 0 8px 0 14px;
  cursor: pointer;
  user-select: none;
}
.section-header:hover .section-title { color: var(--text-secondary); }

.section-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-tertiary);
  flex: 1;
  letter-spacing: 0.01em;
}

.section-header-right {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.icon-action {
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  border-radius: var(--radius-sm);
  transition: background 0.1s, color 0.1s;
}
.icon-action:hover { background: var(--bg-hover); color: var(--text-secondary); }

.collapse-chevron {
  color: var(--text-tertiary);
  transition: transform 0.2s ease;
  flex-shrink: 0;
  margin-right: 2px;
}
.collapse-chevron.is-collapsed { transform: rotate(-90deg); }

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
.spin { animation: spin 0.7s linear infinite; }

.library-content {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.library-pinned-nav {
  position: relative;
  flex-shrink: 0;
  z-index: 15;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}

.library-main-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-bottom: 8px;
}

.coll-list {
  padding-left: 0;
}

.all-papers-section {
  margin: 0 0 6px;
  padding: 4px 0;
  border-top: 1px solid var(--border-subtle);
  border-bottom: 1px solid var(--border-subtle);
}

/* All Papers + Recently Read read as one group — no divider between them:
   drop the first section's bottom border/margin and the next's top border. */
.all-papers-section:has(+ .all-papers-section) {
  margin-bottom: 0;
  padding-bottom: 0;
  border-bottom: none;
}
.all-papers-section + .all-papers-section {
  padding-top: 0;
  border-top: none;
}
.library-pinned-nav .all-papers-section:last-child {
  margin-bottom: 0;
  border-bottom: none;
}

.all-papers-section .nav-item {
  margin-top: 0;
  margin-bottom: 0;
}
.all-papers-section.collection-root-drop-over .nav-item {
  outline: 1.5px solid color-mix(in srgb, var(--accent) 72%, transparent);
  outline-offset: -2px;
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-primary));
  color: var(--accent);
}

.coll-list.collection-root-drop-over {
  background: color-mix(in srgb, var(--accent) 5%, transparent);
}

.library-main-scroll.collection-root-drop-over {
  background: color-mix(in srgb, var(--accent) 4%, transparent);
}

/* macOS Finder-style nav items */
.nav-item {
  display: flex;
  align-items: center;
  gap: 7px;
  width: calc(100% - 12px);
  margin: 1px 6px;
  padding: 5px 9px;
  font-size: 13px;
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  text-align: left;
  cursor: pointer;
  transition: background 0.1s, color 0.1s;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.nav-item:hover { background: var(--bg-hover); color: var(--text-primary); }
.nav-item.active {
  background: var(--accent);
  color: #fff;
  font-weight: 500;
}
.nav-item.drag-over {
  outline: 1.5px solid color-mix(in srgb, var(--accent) 72%, transparent);
  outline-offset: -2px;
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-primary));
  color: var(--accent);
}

.nav-item .badge {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 500;
  background: rgba(0, 0, 0, 0.10);
  color: var(--text-secondary);
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  min-width: 20px;
  text-align: center;
}
.nav-item.active .badge {
  background: rgba(255, 255, 255, 0.22);
  color: #fff;
}

.nav-item .badge.embed-badge {
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
  min-width: 34px;
  font-variant-numeric: tabular-nums;
}
.nav-item .badge.embed-badge.done {
  background: color-mix(in srgb, #34a853 16%, transparent);
  color: #34a853;
}
.nav-item.active .badge.embed-badge {
  background: rgba(255, 255, 255, 0.28);
  color: #fff;
}

.no-collections {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  padding: 4px 14px 6px 20px;
}

.collection-drag-ghost {
  position: fixed;
  z-index: 100000;
  pointer-events: none;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  max-width: 240px;
  padding: 6px 10px;
  border: 1px solid color-mix(in srgb, var(--accent) 22%, var(--border-subtle));
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 94%, var(--accent));
  color: var(--text-primary);
  box-shadow: 0 10px 26px rgba(15, 23, 42, 0.16);
  font-size: 13px;
  line-height: 1.2;
  white-space: nowrap;
}

.collection-drag-ghost span:last-child {
  overflow: hidden;
  text-overflow: ellipsis;
}

.new-coll-row { padding: 3px 6px 3px 12px; }
.coll-name-input {
  width: 100%;
  font-size: var(--font-size-sm);
  padding: 5px 9px;
  border: 1.5px solid var(--accent);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-primary);
  box-sizing: border-box;
  box-shadow: 0 0 0 3px var(--accent-light);
}

.canvas-rename-input {
  min-width: 0;
  padding: 3px 7px;
  font-size: 13px;
}

.canvas-section {
  flex-shrink: 0;
  position: relative;
  display: flex;
  flex-direction: column;
  border-top: 1px solid var(--border-subtle);
  overflow: hidden;
}

.canvas-section > .section-header {
  border-bottom: 1px solid var(--border-subtle);
}

.canvas-resize-handle {
  position: absolute;
  left: 0;
  right: 0;
  top: -4px;
  height: 9px;
  cursor: row-resize;
  z-index: 2;
}
.canvas-resize-handle::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  top: 4px;
  height: 1px;
  background: transparent;
  transition: background 0.12s;
}
.canvas-resize-handle:hover::after {
  background: color-mix(in srgb, var(--accent) 45%, transparent);
}

.canvas-list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-bottom: 4px;
}

.canvas-list.mode-list {
  flex: initial;
  min-height: auto;
  overflow: visible;
  padding: 2px 0 8px;
}

.canvas-name-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tags-panel {
  flex-shrink: 0;
  position: relative;
  display: flex;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
  padding: 10px 12px 9px;
  border-top: 1px solid var(--border-subtle);
}

.tags-panel.tags-panel-fixed {
  max-height: min(48vh, 360px);
}

.tags-resize-handle {
  position: absolute;
  left: 0;
  right: 0;
  top: -4px;
  height: 9px;
  cursor: row-resize;
  z-index: 2;
}
.tags-resize-handle::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  top: 4px;
  height: 1px;
  background: transparent;
  transition: background 0.12s;
}
.tags-resize-handle:hover::after {
  background: color-mix(in srgb, var(--accent) 45%, transparent);
}

.tags-title {
  flex-shrink: 0;
  display: block;
  margin-bottom: 6px;
}

.tag-cloud {
  flex: 0 1 auto;
  min-height: 0;
  display: flex;
  flex-wrap: wrap;
  align-content: flex-start;
  gap: 6px;
  min-width: 0;
  overflow-y: auto;
  overflow-x: hidden;
  padding-right: 2px;
}

.tag-chip {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  max-width: 100%;
  min-width: 0;
  padding: 2px 3px 2px 7px;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-secondary);
  border: 0;
  font-size: 12px;
  line-height: 1.25;
  white-space: nowrap;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.tag-chip-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 11px;
  height: 11px;
  flex-shrink: 0;
  margin-right: 2px;
}

.tag-chip-text {
  overflow: hidden;
  text-overflow: ellipsis;
}

.tag-chip:hover {
  background: color-mix(in srgb, var(--text-primary) 6%, transparent);
  color: var(--text-primary);
}

.tag-chip.active {
  background: var(--accent-light);
  color: var(--accent);
  font-weight: 500;
}

.tag-chip-delete {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  padding: 0;
  margin: 0;
  border: 0;
  border-radius: var(--radius-pill);
  background: transparent;
  color: inherit;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.12s, background 0.1s, color 0.1s;
}

.tag-chip:hover .tag-chip-delete,
.tag-chip-delete:focus-visible {
  opacity: 1;
}

.tag-chip-delete:hover {
  background: color-mix(in srgb, var(--danger) 15%, transparent);
  color: var(--danger);
}

.sidebar-footer {
  flex-shrink: 0;
  padding: 7px 0 9px;
  border-top: 1px solid var(--border-subtle);
}

.settings-nav-btn {
  display: flex;
  align-items: center;
  gap: 7px;
  width: calc(100% - 12px);
  margin: 0 6px;
  padding: 6px 9px;
  font-size: 13px;
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  text-align: left;
  transition: background 0.1s, color 0.1s;
}
.settings-nav-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.settings-nav-btn svg { flex-shrink: 0; }

.settings-icon-wrap {
  position: relative;
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

.update-dot {
  position: absolute;
  top: -3px;
  right: -3px;
  width: 7px;
  height: 7px;
  background: #ef4444;
  border-radius: 50%;
  border: 1.5px solid var(--bg-secondary);
}

/* Context menu */
:global(.ctx-menu) {
  position: fixed;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  padding: 5px;
  min-width: 168px;
  z-index: 2000;
}
:global(.ctx-item) {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 6px 10px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  text-align: left;
  border-radius: var(--radius-sm);
  transition: background 0.08s, color 0.08s;
  gap: 6px;
}
:global(.ctx-item:hover) { background: var(--accent); color: #fff; }
:global(.ctx-item.danger) { color: #e53e3e; }
:global(.ctx-item.danger:hover) { background: #e53e3e; color: #fff; }
:global(.ctx-sep) { height: 1px; background: var(--border-subtle); margin: 4px 0; }

:global(.emoji-picker-backdrop) {
  position: fixed;
  inset: 0;
  z-index: 3000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.18);
  backdrop-filter: blur(2px);
}

:global(.emoji-picker-modal) {
  width: 430px;
  max-width: calc(100vw - 40px);
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 14px;
}

:global(.emoji-picker-header) {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

:global(.emoji-picker-title) {
  font-size: 15px;
  font-weight: 700;
  color: var(--text-primary);
}

:global(.emoji-picker-subtitle) {
  max-width: 320px;
  margin-top: 2px;
  font-size: 12px;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

:global(.emoji-picker-close) {
  width: 26px;
  height: 26px;
  border-radius: var(--radius-md);
  color: var(--text-tertiary);
  display: flex;
  align-items: center;
  justify-content: center;
}
:global(.emoji-picker-close:hover) {
  background: var(--bg-hover);
  color: var(--text-primary);
}

:global(.emoji-search) {
  width: 100%;
  box-sizing: border-box;
  padding: 7px 10px;
  margin-bottom: 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  outline: none;
}
:global(.emoji-search:focus) {
  border-color: var(--accent);
}

:global(.emoji-grid) {
  display: grid;
  grid-template-columns: repeat(10, 1fr);
  gap: 6px;
}

:global(.emoji-empty) {
  grid-column: 1 / -1;
  padding: 24px 0;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

:global(.emoji-option) {
  height: 34px;
  border-radius: var(--radius-md);
  font-size: 20px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--bg-secondary);
  transition: background 0.1s, box-shadow 0.1s, transform 0.1s;
}
:global(.emoji-option:hover),
:global(.emoji-option.active) {
  background: var(--accent-light);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
}
:global(.emoji-option:active) {
  transform: scale(0.96);
}

:global(.emoji-pager) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-top: 12px;
  font-size: 12px;
  color: var(--text-tertiary);
}

:global(.emoji-pager-btn) {
  padding: 4px 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-pill);
  color: var(--text-secondary);
  background: var(--bg-secondary);
  font-size: 12px;
}
:global(.emoji-pager-btn:hover:not(:disabled)) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
:global(.emoji-pager-btn:disabled) {
  opacity: 0.35;
  cursor: default;
}

:global(.emoji-manual) {
  display: flex;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--border-subtle);
}

:global(.emoji-input) {
  flex: 1;
  min-width: 0;
  height: 30px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-primary);
  padding: 0 10px;
  font-size: 14px;
  outline: none;
}
:global(.emoji-input:focus) {
  border-color: var(--accent);
  background: var(--bg-primary);
  box-shadow: 0 0 0 3px var(--accent-light);
}

:global(.emoji-save-btn) {
  height: 30px;
  padding: 0 14px;
  border-radius: var(--radius-md);
  background: var(--accent);
  color: #fff;
  font-size: 13px;
  font-weight: 600;
}
:global(.emoji-save-btn:disabled) {
  opacity: 0.35;
  cursor: default;
}
</style>
