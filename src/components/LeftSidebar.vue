<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useLibraryStore } from '../stores/library'
import { useSelectionStore } from '../stores/selection'
import { useCollectionsStore } from '../stores/collections'
import { useCanvasStore } from '../stores/canvas'
import CollectionNode from './CollectionNode.vue'
import type { Collection, NavItem } from '../types'
import { updateStore } from '../stores/update'

const { t } = useI18n()
const library = useLibraryStore()
const selection = useSelectionStore()
const collectionsStore = useCollectionsStore()
const canvasStore = useCanvasStore()

const showSettings = defineModel<boolean>('showSettings', { default: false })
const emit = defineEmits<{ 'open-canvas': [canvasId: string] }>()

const expanded = ref<Set<string>>(new Set())
const libraryCollapsed = ref(false)
const EXPANDED_COLLECTIONS_KEY_PREFIX = 'argus:sidebar:expanded-collections'
const CANVAS_HEIGHT_KEY = 'argus:sidebar:canvas-height'
const canvasPanelHeight = ref(loadCanvasPanelHeight())
const canvasCollapsed = ref(false)
const showNewCanvasInput = ref(false)
const newCanvasName = ref('')
const TAGS_HEIGHT_KEY = 'argus:sidebar:tags-height'
const tagsPanelHeight = ref(loadTagsPanelHeight())

function loadCanvasPanelHeight() {
  try {
    const raw = Number(localStorage.getItem(CANVAS_HEIGHT_KEY))
    if (Number.isFinite(raw) && raw > 0) return Math.min(400, Math.max(72, raw))
  } catch {}
  return 160
}
const EMOJI_PAGE_SIZE = 60
const COLLECTION_EMOJIS = [
  '📚', '📖', '📘', '📙', '📗', '📕', '📓', '📔', '📒', '🗂️',
  '🗃️', '🗄️', '📁', '📂', '🧾', '📝', '📄', '📑', '📰', '🏷️',
  '🔖', '📌', '📍', '🧷', '✏️', '🖊️', '🖋️', '🖍️', '📐', '📏',
  '🔎', '🔍', '🧠', '💡', '🎯', '🧩', '🧭', '🧪', '🔬', '🔭',
  '⚗️', '🧬', '🧮', '📊', '📈', '📉', '💻', '⌨️', '🖥️', '🖨️',
  '⚙️', '🛠️', '🔧', '🔨', '⛓️', '🧱', '🧰', '💾', '💿', '📡',
  '🚀', '🛰️', '🌐', '🗺️', '🧳', '🏛️', '🏫', '🏢', '🏗️', '🏁',
  '⭐', '🌟', '✨', '💫', '🔥', '🌱', '🌿', '☘️', '🌲', '🌳',
  '🌊', '☀️', '🌙', '☁️', '⚡', '❄️', '🌈', '💎', '🔮', '🪄',
  '❤️', '🧡', '💛', '💚', '💙', '💜', '🖤', '🤍', '🤎', '💭',
  '🕯️', '🪞', '🧵', '🪡', '🎨', '🖼️', '🎼', '🎧', '🎬', '🎮',
  '🏆', '🥇', '🎲', '🃏', '♟️', '🧿', '🪬', '🔐', '🗝️', '🚦',
  '✅', '☑️', '✔️', '❌', '⭕', '❗', '❓', '➕', '➖', '➗',
  '🔴', '🟠', '🟡', '🟢', '🔵', '🟣', '🟤', '⚪', '⚫', '🔺',
  '🔻', '🔸', '🔹', '🔶', '🔷', '◼️', '◻️', '◾', '◽', '⬛',
  '⬜', '🟦', '🟩', '🟨', '🟧', '🟥', '🟪', '🟫', '🔳', '🔲',
]

function loadTagsPanelHeight() {
  try {
    const raw = Number(localStorage.getItem(TAGS_HEIGHT_KEY))
    if (Number.isFinite(raw) && raw > 0) return Math.min(360, Math.max(72, raw))
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
})

function select(item: NavItem) {
  selection.selectNav(item)
}

function toggleTag(tag: string) {
  selection.toggleTagFilter(tag)
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

// ── New collection ─────────────────────────────────────────────────────────────
const showNewInput = ref(false)
const newCollName = ref('')
const newCollParent = ref<string | undefined>(undefined)

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

// ── Emoji picker ─────────────────────────────────────────────────────────────
const emojiPicker = ref<{ col: Collection } | null>(null)
const emojiDraft = ref('')
const emojiPage = ref(0)
const emojiPageCount = computed(() => Math.ceil(COLLECTION_EMOJIS.length / EMOJI_PAGE_SIZE))
const visibleEmojis = computed(() => {
  const start = emojiPage.value * EMOJI_PAGE_SIZE
  return COLLECTION_EMOJIS.slice(start, start + EMOJI_PAGE_SIZE)
})

function openEmojiPicker(col: Collection) {
  closeCtx()
  emojiPicker.value = { col }
  emojiDraft.value = col.emoji ?? ''
  const currentIndex = COLLECTION_EMOJIS.indexOf(col.emoji ?? '')
  emojiPage.value = currentIndex >= 0 ? Math.floor(currentIndex / EMOJI_PAGE_SIZE) : 0
  nextTick(() => (document.getElementById('collection-emoji-input') as HTMLInputElement | null)?.focus())
}

function closeEmojiPicker() {
  emojiPicker.value = null
  emojiDraft.value = ''
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
  try {
    await canvasStore.openCanvas(canvasId)
    emit('open-canvas', canvasId)
  } catch (e) {
    console.error('Open canvas:', e)
  }
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

// ── Context menu (collections) ────────────────────────────────────────────────
const ctxMenu = ref<{ x: number; y: number; col: Collection } | null>(null)

function openCtx(e: MouseEvent, col: Collection) {
  e.preventDefault()
  libCtxMenu.value = null
  ctxMenu.value = { x: e.clientX, y: e.clientY, col }
}

// ── Context menu (All Papers / library root) ──────────────────────────────────
const libCtxMenu = ref<{ x: number; y: number } | null>(null)

function openLibCtx(e: MouseEvent) {
  e.preventDefault()
  ctxMenu.value = null
  libCtxMenu.value = { x: e.clientX, y: e.clientY }
}

function closeCtx() {
  ctxMenu.value = null
  libCtxMenu.value = null
}

// ── Drag-drop targets (driven by pointer-based drag in PaperList) ─────────────
const dragOverId = ref<string | null>(null)

function onPaperDragOver(e: Event) {
  dragOverId.value = (e as CustomEvent<{ collectionId: string | null }>).detail.collectionId
}

onMounted(() => document.addEventListener('argus-paper-drag-over', onPaperDragOver))

// ── Tags panel resize ─────────────────────────────────────────────────────────
let tagResizeStartY = 0
let tagResizeStartH = 0
let isResizingTags = false

function clampTagsHeight(height: number) {
  const max = Math.min(360, Math.max(120, window.innerHeight * 0.48))
  return Math.min(max, Math.max(72, height))
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
  document.removeEventListener('argus-paper-drag-over', onPaperDragOver)
  window.removeEventListener('mousemove', onResizeTagsMove)
  window.removeEventListener('mouseup', stopResizeTags)
  window.removeEventListener('mousemove', onResizeCanvasMove)
  window.removeEventListener('mouseup', stopResizeCanvas)
})
</script>

<template>
  <div class="left-sidebar" @click="closeCtx">
    <div class="sidebar-scroll">
      <!-- Library section (collapsible) -->
      <div class="section">
        <!-- Section header: click to collapse, + to new collection -->
        <div class="section-header" @click.stop="libraryCollapsed = !libraryCollapsed">
          <span class="section-title">{{ t('sidebar.library') }}</span>
          <div class="section-header-right">
            <button class="icon-action" :title="t('toolbar.refreshTitle')" :disabled="library.isRefreshing" @click.stop="library.refresh()">
              <svg
                width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                :class="{ spin: library.isRefreshing || library.isLoading }"
              >
                <polyline points="23 4 23 10 17 10"/>
                <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
              </svg>
            </button>
            <button class="icon-action" :title="t('collections.new')" @click.stop="startNew()">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="12" y1="5" x2="12" y2="19"/>
                <line x1="5" y1="12" x2="19" y2="12"/>
              </svg>
            </button>
            <svg
              width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
              class="collapse-chevron"
              :class="{ 'is-collapsed': libraryCollapsed }"
            >
              <polyline points="6 9 12 15 18 9"/>
            </svg>
          </div>
        </div>

        <template v-if="!libraryCollapsed">
          <!-- All Papers -->
          <div class="all-papers-section">
            <button
              class="nav-item"
              :class="{ active: selection.activeNav === 'all' }"
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

          <!-- Collections directly below All Papers -->
          <div class="coll-list">
            <!-- New top-level collection input -->
            <div v-if="showNewInput && !newCollParent" class="new-coll-row">
              <input
                id="new-coll-input"
                v-model="newCollName"
                class="coll-name-input"
                :placeholder="t('collections.namePlaceholder')"
                @keydown.enter="submitNew"
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
              :show-new-input="showNewInput"
              :new-coll-parent="newCollParent"
              :new-coll-name="newCollName"
              @toggle-expand="toggleExpand"
              @open-ctx="openCtx"
              @start-rename="startRename"
              @submit-rename="submitRename"
              @delete="deleteCollection"
              @start-new="startNew"
              @submit-new="submitNew"
              @update:renameValue="renameValue = $event"
              @update:newCollName="newCollName = $event"
              @update:showNewInput="showNewInput = $event"
            />
          </div>
        </template>
      </div>
    </div>

    <!-- Canvas / 论文关系图谱 section -->
    <div
      v-if="library.currentPath"
      class="canvas-section"
      :style="canvasCollapsed ? {} : { height: `${canvasPanelHeight}px` }"
    >
      <div v-if="!canvasCollapsed" class="canvas-resize-handle" @mousedown.stop.prevent="startResizeCanvas" />

      <div class="section-header" @click.stop="canvasCollapsed = !canvasCollapsed">
        <span class="section-title">{{ t('sidebar.canvas') }}</span>
        <div class="section-header-right">
          <button class="icon-action" :title="t('toolbar.refreshTitle')" @click.stop="canvasStore.loadList()">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
              :class="{ spin: canvasStore.loading }">
              <polyline points="23 4 23 10 17 10"/>
              <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/>
            </svg>
          </button>
          <button class="icon-action" :title="t('canvas.newCanvas')" @click.stop="startNewCanvas()">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/>
              <line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
          </button>
          <svg
            width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
            class="collapse-chevron"
            :class="{ 'is-collapsed': canvasCollapsed }"
          >
            <polyline points="6 9 12 15 18 9"/>
          </svg>
        </div>
      </div>

      <template v-if="!canvasCollapsed">
        <div class="canvas-list">
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

          <button
            v-for="cv in canvasStore.canvasList"
            :key="cv.id"
            class="nav-item"
            :class="{ active: canvasStore.currentCanvas?.id === cv.id }"
            @click="openSpecificCanvas(cv.id)"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="8" cy="8" r="2"/><circle cx="16" cy="8" r="2"/>
              <circle cx="12" cy="17" r="2"/>
              <line x1="9.8" y1="8.8" x2="13.2" y2="15.4"/>
              <line x1="14.2" y1="8.8" x2="10.8" y2="15.4"/>
              <line x1="10" y1="8" x2="14" y2="8"/>
            </svg>
            <span class="canvas-name-text">{{ cv.name }}</span>
            <span v-if="cv.node_count > 0" class="badge">{{ cv.node_count }}</span>
          </button>
        </div>
      </template>
    </div>

    <!-- Tags section -->
    <div
      v-if="library.allTags.length > 0"
      class="tags-panel"
      :style="{ height: `${tagsPanelHeight}px` }"
    >
      <div class="tags-resize-handle" @mousedown.stop.prevent="startResizeTags" />
      <div class="section-title tags-title">{{ t('sidebar.tags') }}</div>
      <div class="tag-cloud">
        <button
          v-for="tag in library.allTags"
          :key="tag"
          class="tag-chip"
          :class="{ active: selection.tagFilter === tag }"
          :title="tag"
          @click="toggleTag(tag)"
        >
          {{ tag }}
        </button>
      </div>
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
        <button class="ctx-item" @click="openEmojiPicker(ctxMenu!.col)">
          {{ t('collections.setEmoji') }}
        </button>
        <button class="ctx-item" @click="startNew(ctxMenu!.col.id); closeCtx()">
          {{ t('collections.newSub') }}
        </button>
        <button class="ctx-item" @click="openCollectionInFinder(ctxMenu!.col); closeCtx()">
          {{ t('collections.openInFinder') }}
        </button>
        <div class="ctx-sep" />
        <button class="ctx-item danger" @click="deleteCollection(ctxMenu!.col); closeCtx()">
          {{ t('collections.delete') }}
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

          <div class="emoji-grid" role="listbox">
            <button
              v-for="emoji in visibleEmojis"
              :key="emoji"
              class="emoji-option"
              :class="{ active: emoji === emojiDraft }"
              @click="saveCollectionEmoji(emoji)"
            >
              {{ emoji }}
            </button>
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
  overflow-y: auto;
  padding: 0 0 10px;
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

.coll-list {
  padding-left: 0;
}

.all-papers-section {
  margin: 0 0 6px;
  padding: 4px 0;
  border-top: 1px solid var(--border-subtle);
  border-bottom: 1px solid var(--border-subtle);
}

.all-papers-section .nav-item {
  margin-top: 0;
  margin-bottom: 0;
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

.no-collections {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  padding: 4px 14px 6px 20px;
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
  min-height: 72px;
  overflow-y: auto;
  overflow-x: hidden;
  padding: 13px 12px 10px;
  border-top: 1px solid var(--border-subtle);
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
  display: block;
  margin-bottom: 7px;
}

.tag-cloud {
  display: flex;
  flex-wrap: wrap;
  align-content: flex-start;
  gap: 6px;
  min-width: 0;
}

.tag-chip {
  display: inline-flex;
  align-items: center;
  max-width: 100%;
  min-width: 0;
  padding: 2px 5px;
  border-radius: var(--radius-pill);
  background: transparent;
  color: var(--text-secondary);
  border: 0;
  font-size: 12px;
  line-height: 1.25;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
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

:global(.emoji-grid) {
  display: grid;
  grid-template-columns: repeat(10, 1fr);
  gap: 6px;
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
