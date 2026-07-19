<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onUnmounted, watch, Teleport } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  libraries,
  snippets,
  deleteSnippet,
  updateSnippet,
  moveSnippet,
  type Snippet,
} from '../stores/snippetLibrary'
import { useRagStore } from '../stores/rag'

const props = defineProps<{ libraryId: string }>()
const emit = defineEmits<{
  'open-paper': [slug: string, page: number, title: string]
  'open-settings': [section?: 'rag']
}>()
const { t } = useI18n()
const ragStore = useRagStore()

const searchQuery = ref('')

// ── Snippet embedding (RAG vectorization) ───────────────────────────────────────
// Scoped to the current library: the counter shows this library's embedded/total
// and the button embeds only this library's unembedded snippets via
// `embed_library_snippets`.
const embeddedCount = ref(0)
const syncing = ref(false)
const syncProgress = ref({ done: 0, total: 0, failed: 0 })
const totalCount = computed(() => snippets.value.filter(s => s.libraryId === props.libraryId).length)
const unembeddedCount = computed(() => Math.max(0, totalCount.value - embeddedCount.value))

async function loadEmbeddedCount() {
  try {
    embeddedCount.value = await invoke<number>('get_library_embedded_count', { libraryId: props.libraryId })
  } catch {
    embeddedCount.value = 0
  }
}

let unlistenEmbedProgress: UnlistenFn | null = null

async function syncEmbeddings() {
  if (syncing.value || !ragStore.isConfigured || unembeddedCount.value === 0) return
  syncing.value = true
  syncProgress.value = { done: 0, total: unembeddedCount.value, failed: 0 }
  unlistenEmbedProgress = await listen<{ done: number; failed: number; total: number }>(
    'snippet-embed-progress',
    (ev) => {
      syncProgress.value = { done: ev.payload.done, total: ev.payload.total, failed: ev.payload.failed }
    },
  )
  try {
    const [done, failed] = await invoke<[number, number]>('embed_library_snippets', { libraryId: props.libraryId })
    syncProgress.value = { done, total: done + failed, failed }
    await loadEmbeddedCount()
  } catch {
    /* surfaced to the user via the counter staying unchanged */
  } finally {
    unlistenEmbedProgress?.()
    unlistenEmbedProgress = null
    syncing.value = false
  }
}

onMounted(() => {
  if (!ragStore.loaded) ragStore.load()
  loadEmbeddedCount()
})

// The component instance is reused across libraries (no :key), so reload the
// embedded count whenever the active library changes.
watch(() => props.libraryId, () => { loadEmbeddedCount() })

onUnmounted(() => {
  unlistenEmbedProgress?.()
})

const library = computed(() => libraries.value.find(l => l.id === props.libraryId))

const items = computed(() => {
  const q = searchQuery.value.toLowerCase().trim()
  const list = snippets.value
    .filter(s => s.libraryId === props.libraryId)
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
  if (!q) return list
  return list.filter(s =>
    s.text.toLowerCase().includes(q) ||
    s.paperTitle.toLowerCase().includes(q) ||
    s.note.toLowerCase().includes(q) ||
    s.tags.some(tag => tag.toLowerCase().includes(q))
  )
})

function openPaper(s: Snippet) {
  emit('open-paper', s.paperId, s.page, s.paperTitle)
}

async function handleDelete(s: Snippet) {
  if (!window.confirm(t('snippets.deleteConfirm'))) return
  await deleteSnippet(s.id)
}

function truncate(text: string, max = 120) {
  return text.length > max ? text.slice(0, max) + '…' : text
}

// ── Drag to move snippet between libraries (pointer-based, no HTML5 DnD) ────────
const dragSnippet = ref<Snippet | null>(null)
const dragPos = ref({ x: 0, y: 0 })
const draggingSnippetId = ref<string | null>(null)

function isInteractiveDragTarget(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false
  return el.closest('button, input, textarea, .tag-editor, .editable-cell') !== null
}

function onSnippetMouseDown(e: MouseEvent, s: Snippet) {
  if (e.button !== 0) return
  if (isInteractiveDragTarget(e.target)) return
  const startX = e.clientX
  const startY = e.clientY
  let dragging = false

  function findLibraryId(x: number, y: number): string | null {
    for (const el of document.elementsFromPoint(x, y)) {
      const id = (el as HTMLElement).dataset?.snippetLibraryId
      if (id) return id
    }
    return null
  }

  function onMove(ev: MouseEvent) {
    if (!dragging && Math.hypot(ev.clientX - startX, ev.clientY - startY) < 6) return
    if (!dragging) {
      dragging = true
      dragSnippet.value = s
      draggingSnippetId.value = s.id
      document.body.style.cursor = 'grabbing'
    }
    dragPos.value = { x: ev.clientX + 14, y: ev.clientY + 10 }
    const libraryId = findLibraryId(ev.clientX, ev.clientY)
    document.dispatchEvent(new CustomEvent('argus-snippet-drag-over', { detail: { libraryId } }))
  }

  async function onUp(ev: MouseEvent) {
    window.removeEventListener('mousemove', onMove)
    window.removeEventListener('mouseup', onUp)
    document.body.style.cursor = ''
    const wasDragging = dragging
    if (wasDragging) {
      dragSnippet.value = null
      draggingSnippetId.value = null
      document.dispatchEvent(new CustomEvent('argus-snippet-drag-over', { detail: { libraryId: null } }))
      const targetLibraryId = findLibraryId(ev.clientX, ev.clientY)
      if (targetLibraryId && targetLibraryId !== s.libraryId) {
        await moveSnippet(s.id, targetLibraryId)
      }
    }
  }

  window.addEventListener('mousemove', onMove)
  window.addEventListener('mouseup', onUp)
}

// ── Inline note editing ───────────────────────────────────────────────────────
const editingNoteId = ref<string | null>(null)
const editNoteValue = ref('')

function startEditNote(s: Snippet) {
  editingNoteId.value = s.id
  editNoteValue.value = s.note
  nextTick(() => {
    const el = document.getElementById(`note-input-${s.id}`) as HTMLTextAreaElement | null
    el?.focus()
    el?.select()
  })
}

async function commitNote(s: Snippet) {
  if (editingNoteId.value !== s.id) return
  editingNoteId.value = null
  const note = editNoteValue.value.trim()
  if (note !== s.note) await updateSnippet(s.id, { note })
}

function onNoteKeydown(e: KeyboardEvent, s: Snippet) {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); commitNote(s) }
  if (e.key === 'Escape') { editingNoteId.value = null }
}

// ── Inline tag editing ────────────────────────────────────────────────────────
const editingTagsId = ref<string | null>(null)
const editTagsList = ref<string[]>([])
const editTagInput = ref('')

function startEditTags(s: Snippet) {
  editingTagsId.value = s.id
  editTagsList.value = [...s.tags]
  editTagInput.value = ''
  nextTick(() => {
    const el = document.getElementById(`tag-input-${s.id}`) as HTMLInputElement | null
    el?.focus()
  })
}

function commitTagInput() {
  const val = editTagInput.value.replace(/,/g, '').trim()
  if (val && !editTagsList.value.includes(val)) {
    editTagsList.value = [...editTagsList.value, val]
  }
  editTagInput.value = ''
}

function removeEditTag(tag: string) {
  editTagsList.value = editTagsList.value.filter(t => t !== tag)
}

// ── Tag suggestions (distinct tags already used in this library) ───────────────
const editTagFocused = ref(false)
const tagSuggestPos = ref({ x: 0, y: 0, width: 0 })

const libraryTags = computed(() => {
  const all = snippets.value
    .filter(s => s.libraryId === props.libraryId)
    .flatMap(s => s.tags)
  return [...new Set(all)]
})

const tagSuggestions = computed(() => {
  const q = editTagInput.value.trim().toLowerCase()
  return libraryTags.value.filter(tag =>
    !editTagsList.value.includes(tag) && (q === '' || tag.toLowerCase().includes(q))
  )
})

const showTagSuggestions = computed(() =>
  editingTagsId.value !== null && editTagFocused.value && tagSuggestions.value.length > 0
)

// The editor lives in a scrolling table, so the dropdown is teleported to <body>
// and positioned (fixed) just below the input to avoid being clipped.
function updateTagSuggestPos() {
  const id = editingTagsId.value
  if (!id) return
  const chips = document.getElementById(`tag-input-${id}`)?.closest('.tag-edit-chips') as HTMLElement | null
  if (!chips) return
  const r = chips.getBoundingClientRect()
  tagSuggestPos.value = { x: r.left, y: r.bottom + 4, width: r.width }
}

function onTagInputFocus() {
  editTagFocused.value = true
  nextTick(updateTagSuggestPos)
}

function pickEditSuggestion(tag: string) {
  const id = editingTagsId.value
  if (!id) return
  if (!editTagsList.value.includes(tag)) editTagsList.value = [...editTagsList.value, tag]
  editTagInput.value = ''
  const el = document.getElementById(`tag-input-${id}`) as HTMLInputElement | null
  el?.focus() // keep the editor open; suggestion mousedown is prevented from blurring
  nextTick(updateTagSuggestPos)
}

function onTagInputKeydown(e: KeyboardEvent, s: Snippet) {
  // Ignore Enter/comma fired mid-IME-composition (e.g. picking Chinese chars).
  if (e.isComposing || e.keyCode === 229) return
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    commitTagInput()
    nextTick(updateTagSuggestPos)
  } else if (e.key === 'Backspace' && editTagInput.value === '' && editTagsList.value.length > 0) {
    editTagsList.value = editTagsList.value.slice(0, -1)
  } else if (e.key === 'Escape') {
    editingTagsId.value = null
    editTagFocused.value = false
  }
}

async function commitTags(s: Snippet) {
  if (editingTagsId.value !== s.id) return
  commitTagInput()
  editingTagsId.value = null
  editTagFocused.value = false
  const newTags = editTagsList.value
  const changed = JSON.stringify(newTags) !== JSON.stringify(s.tags)
  if (changed) await updateSnippet(s.id, { tags: newTags })
}
</script>

<template>
  <div class="snippet-view">
    <!-- Header -->
    <div class="snippet-header">
      <div class="header-left">
        <span v-if="library?.emoji" class="lib-emoji">{{ library.emoji }}</span>
        <Icon v-else icon="fluent:folder-24-regular" width="16" height="16" />
        <span class="lib-name">{{ library?.name ?? t('snippets.title') }}</span>
        <span class="item-count">{{ items.length }}</span>
      </div>

      <!-- Embedding (RAG vectorization) control -->
      <div class="embed-control">
        <button
          v-if="!ragStore.isConfigured"
          class="embed-config-btn"
          :title="t('snippetLibrary.embedConfigTip')"
          @click="emit('open-settings', 'rag')"
        >
          <Icon icon="fluent:database-24-regular" width="12" height="12" />
          {{ t('snippetLibrary.embedConfig') }}
        </button>
        <template v-else>
          <span v-if="syncing" class="embed-progress">{{ syncProgress.done }}/{{ syncProgress.total }}</span>
          <button
            class="embed-refresh-btn"
            :class="{ refreshing: syncing }"
            :title="t('snippetLibrary.embedRefreshTip')"
            :disabled="syncing"
            @click="loadEmbeddedCount"
          >
            <Icon icon="fluent:arrow-sync-24-regular" width="15" height="15" />
          </button>
          <div class="embed-counter" :title="t('snippetLibrary.embedCounterTip')">
            <Icon icon="fluent:database-24-regular" width="11" height="11" />
            <span>{{ embeddedCount }}/{{ totalCount }}</span>
          </div>
          <button
            class="embed-sync-btn"
            :class="{ 'all-done': unembeddedCount === 0 && totalCount > 0 }"
            :title="unembeddedCount > 0 ? t('snippetLibrary.embedNTip', { n: unembeddedCount }) : t('snippetLibrary.embedDone')"
            :disabled="syncing || unembeddedCount === 0"
            @click="syncEmbeddings"
          >
            <Icon v-if="unembeddedCount > 0" icon="fluent:cloud-arrow-up-24-regular" width="11" height="11" />
            <Icon v-else icon="fluent:checkmark-24-regular" width="11" height="11" />
            {{ syncing ? t('snippetLibrary.embedding') : unembeddedCount > 0 ? t('snippetLibrary.embedN', { n: unembeddedCount }) : (totalCount > 0 ? t('snippetLibrary.embedDone') : t('snippetLibrary.embedNone')) }}
          </button>
        </template>
      </div>

      <div class="search-wrap">
        <Icon class="search-icon" icon="fluent:search-24-regular" width="13" height="13" />
        <input v-model="searchQuery" class="search-input" :placeholder="t('snippets.search')" />
      </div>
    </div>

    <!-- Empty state -->
    <div v-if="items.length === 0" class="empty-state">
      <Icon icon="fluent:folder-add-24-regular" width="40" height="40" style="opacity:0.35" />
      <p class="empty-title">{{ t('snippets.noSnippets') }}</p>
      <p class="empty-hint">{{ t('snippets.addHint') }}</p>
    </div>

    <!-- Table -->
    <div v-else class="table-wrap">
      <table class="snippet-table">
        <thead>
          <tr>
            <th class="col-tags">{{ t('snippets.tags') }}</th>
            <th class="col-content">{{ t('snippets.sourceText') }}</th>
            <th class="col-paper">{{ t('snippets.sourcePaper') }}</th>
            <th class="col-note">{{ t('snippets.note') }}</th>
            <th class="col-action"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="s in items" :key="s.id">
            <tr
              class="snippet-row"
              :class="{ 'is-dragging': draggingSnippetId === s.id }"
              @mousedown="onSnippetMouseDown($event, s)"
              @dblclick="openPaper(s)"
            >

              <!-- Tags cell -->
              <td class="col-tags" @click.stop>
                <!-- Editing mode -->
                <div v-if="editingTagsId === s.id" class="tag-editor" @mousedown.stop>
                  <div class="tag-edit-chips">
                    <span v-for="tag in editTagsList" :key="tag" class="tag-chip editable">
                      {{ tag }}
                      <button class="tag-remove" @mousedown.prevent="removeEditTag(tag)">×</button>
                    </span>
                    <input
                      :id="`tag-input-${s.id}`"
                      v-model="editTagInput"
                      class="tag-inline-input"
                      :placeholder="t('snippetLibrary.tagPlaceholder')"
                      @keydown="onTagInputKeydown($event, s)"
                      @focus="onTagInputFocus"
                      @blur="commitTags(s)"
                    />
                  </div>
                </div>
                <!-- View mode -->
                <div v-else class="tag-list editable-cell" :title="t('snippetLibrary.editTags')" @click="startEditTags(s)">
                  <span v-for="tag in s.tags" :key="tag" class="tag-chip">{{ tag }}</span>
                  <span v-if="s.tags.length === 0" class="no-tag placeholder">{{ t('snippetLibrary.addTag') }}</span>
                </div>
              </td>

              <!-- Content cell (read-only) -->
              <td class="col-content">
                <span
                  class="content-preview"
                  :style="s.color ? { borderLeft: `3px solid ${s.color}`, paddingLeft: '7px', background: s.color + '22' } : {}"
                >{{ truncate(s.text) }}</span>
              </td>

              <!-- Paper cell (read-only) -->
              <td class="col-paper">
                <span class="paper-title">{{ s.paperTitle }}</span>
                <span class="page-num">P{{ s.page }}</span>
              </td>

              <!-- Note cell -->
              <td class="col-note" @click.stop>
                <!-- Editing mode -->
                <textarea
                  v-if="editingNoteId === s.id"
                  :id="`note-input-${s.id}`"
                  v-model="editNoteValue"
                  class="note-input"
                  rows="2"
                  @keydown="onNoteKeydown($event, s)"
                  @blur="commitNote(s)"
                />
                <!-- View mode -->
                <span
                  v-else
                  class="note-preview editable-cell"
                  :class="{ placeholder: !s.note }"
                  :title="t('snippetLibrary.editNote')"
                  @click="startEditNote(s)"
                >{{ s.note || t('snippetLibrary.addNote') }}</span>
              </td>

              <!-- Delete -->
              <td class="col-action" @click.stop>
                <button class="del-btn" :title="t('hl.delete')" @click="handleDelete(s)">
                  <Icon icon="fluent:delete-24-regular" width="13" height="13" />
                </button>
              </td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>

    <!-- Drag ghost -->
    <Teleport to="body">
      <div
        v-if="dragSnippet"
        class="snippet-drag-ghost"
        :style="{ left: dragPos.x + 'px', top: dragPos.y + 'px' }"
      >{{ truncate(dragSnippet.text, 80) }}</div>
    </Teleport>

    <!-- Tag suggestions (teleported so the scrolling table doesn't clip it) -->
    <Teleport to="body">
      <div
        v-if="showTagSuggestions"
        class="tag-suggestions-pop"
        :style="{ left: `${tagSuggestPos.x}px`, top: `${tagSuggestPos.y}px`, minWidth: `${tagSuggestPos.width}px` }"
      >
        <button
          v-for="sug in tagSuggestions"
          :key="sug"
          class="tag-sug-item"
          @mousedown.prevent="pickEditSuggestion(sug)"
        >
          {{ sug }}
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.snippet-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.snippet-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 20px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
}

.lib-emoji { font-size: 18px; }

.lib-name {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
}

.item-count {
  font-size: 12px;
  color: var(--text-tertiary);
  background: var(--bg-secondary);
  border-radius: 10px;
  padding: 1px 7px;
}

.search-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: 6px;
  padding: 5px 10px;
}

.search-icon { color: var(--text-tertiary); flex-shrink: 0; }

.search-input {
  border: none;
  background: transparent;
  outline: none;
  font-size: 13px;
  color: var(--text-primary);
  width: 160px;
}
.search-input::placeholder { color: var(--text-tertiary); }

/* Embedding (RAG vectorization) control */
.embed-control {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-left: auto;
  flex-shrink: 0;
}

.embed-config-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 11px;
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.embed-config-btn:hover { background: var(--bg-hover); color: var(--text-secondary); }

.embed-progress {
  font-size: 12px;
  font-weight: 500;
  color: var(--accent);
  min-width: 36px;
  text-align: center;
}

.embed-refresh-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  min-width: 30px;
  height: 30px;
  padding: 0;
  line-height: 0;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  background: transparent;
  border: none;
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.embed-refresh-btn svg { display: block; }
.embed-refresh-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
.embed-refresh-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.embed-refresh-btn.refreshing svg { animation: embed-spin 0.7s linear infinite; }

@keyframes embed-spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.embed-counter {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 3px 8px;
  border-radius: var(--radius-pill);
  font-size: 11px;
  font-weight: 600;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, transparent);
  flex-shrink: 0;
  user-select: none;
}

.embed-sync-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 6px 11px;
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 600;
  color: #fff;
  background: var(--accent);
  border: none;
  cursor: pointer;
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s, opacity 0.12s;
}
.embed-sync-btn:hover:not(:disabled) { background: var(--accent-hover); }
.embed-sync-btn.all-done,
.embed-sync-btn:disabled {
  color: var(--text-tertiary);
  background: var(--bg-secondary);
  cursor: default;
  opacity: 0.75;
}

/* Empty state */
.empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px;
  color: var(--text-secondary);
}

.empty-title {
  font-size: 15px;
  font-weight: 500;
  margin: 4px 0 0;
}

.empty-hint {
  font-size: 13px;
  color: var(--text-tertiary);
  text-align: center;
}

/* Table */
.table-wrap {
  flex: 1;
  overflow-y: auto;
  padding: 0 16px 16px;
}

.snippet-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.snippet-table th {
  text-align: left;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-tertiary);
  padding: 10px 8px 8px;
  border-bottom: 1px solid var(--border-subtle);
  position: sticky;
  top: 0;
  background: var(--bg-primary);
  z-index: 1;
}

.snippet-row {
  transition: background 0.1s;
  cursor: pointer;
}
.snippet-row:hover { background: var(--bg-hover); }

.snippet-row td {
  padding: 9px 8px;
  border-bottom: 1px solid var(--border-subtle);
  vertical-align: middle;
}

.col-tags { width: 140px; }
.col-content { }
.col-paper { width: 180px; }
.col-note { width: 160px; }
.col-action { width: 36px; text-align: center; }

/* Editable cell hover hint */
.editable-cell {
  cursor: text;
  border-radius: 4px;
  padding: 2px 4px;
  margin: -2px -4px;
  transition: background 0.1s;
  display: block;
  min-height: 22px;
}
.snippet-row:hover .editable-cell:hover {
  background: color-mix(in srgb, var(--accent) 8%, transparent);
  outline: 1px dashed color-mix(in srgb, var(--accent) 35%, transparent);
}

.placeholder {
  color: var(--text-tertiary) !important;
  font-style: italic;
}

/* Tags */
.tag-list {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
}

.tag-chip {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 1px 7px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  font-size: 11px;
  white-space: nowrap;
}

.tag-chip.editable { padding: 1px 4px 1px 7px; }

.tag-remove {
  font-size: 13px;
  line-height: 1;
  color: var(--accent);
  opacity: 0.6;
  padding: 0 1px;
  border-radius: 50%;
}
.tag-remove:hover { opacity: 1; background: color-mix(in srgb, var(--accent) 20%, transparent); }

.no-tag { color: var(--text-tertiary); }

/* Tag editor */
.tag-editor {
  min-height: 28px;
}

.tag-edit-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  align-items: center;
  padding: 3px 5px;
  border: 1px solid var(--accent);
  border-radius: 6px;
  background: var(--bg-primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 15%, transparent);
  min-height: 28px;
}

.tag-inline-input {
  border: none;
  outline: none;
  background: transparent;
  font-size: 12px;
  color: var(--text-primary);
  min-width: 60px;
  flex: 1;
}
.tag-inline-input::placeholder { color: var(--text-tertiary); }

/* Note */
.content-preview {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  line-height: 1.5;
  color: var(--text-primary);
}

.note-preview {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  color: var(--text-secondary);
}

.note-input {
  width: 100%;
  resize: none;
  border: 1px solid var(--accent);
  border-radius: 6px;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: 13px;
  padding: 4px 6px;
  outline: none;
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 15%, transparent);
  line-height: 1.5;
  font-family: inherit;
}

.paper-title {
  display: block;
  color: var(--text-primary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  max-width: 170px;
}

.page-num {
  font-size: 11px;
  color: var(--text-tertiary);
}

.del-btn {
  color: var(--text-tertiary);
  padding: 4px;
  border-radius: 4px;
  opacity: 0;
  transition: opacity 0.15s;
}
.snippet-row:hover .del-btn { opacity: 1; }
.del-btn:hover { color: #e53935; background: var(--bg-hover); }

.snippet-row.is-dragging {
  opacity: 0.35;
}

:global(.snippet-drag-ghost) {
  position: fixed;
  pointer-events: none;
  z-index: 9999;
  background: var(--bg-primary);
  border: 1.5px solid var(--accent);
  border-radius: 8px;
  padding: 6px 12px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-primary);
  max-width: 220px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.22), 0 2px 6px rgba(0, 0, 0, 0.1);
  animation: snippet-ghost-pop 0.14s cubic-bezier(0.34, 1.56, 0.64, 1) both;
}

@keyframes snippet-ghost-pop {
  from { opacity: 0; transform: scale(0.92); }
  to { opacity: 1; transform: scale(1); }
}

:global(.tag-suggestions-pop) {
  position: fixed;
  z-index: 9999;
  max-width: 320px;
  max-height: 120px;
  overflow-y: auto;
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  padding: 8px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: 8px;
  box-shadow: var(--shadow-lg);
}

:global(.tag-suggestions-pop .tag-sug-item) {
  padding: 3px 10px;
  border-radius: 12px;
  font-size: 12px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
  cursor: pointer;
  transition: background 0.1s, color 0.1s;
}
:global(.tag-suggestions-pop .tag-sug-item:hover) {
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  border-color: var(--accent);
}
</style>
