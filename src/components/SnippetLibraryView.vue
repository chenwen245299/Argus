<script setup lang="ts">
import { ref, computed, nextTick, Teleport } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  libraries,
  snippets,
  deleteSnippet,
  updateSnippet,
  moveSnippet,
  type Snippet,
} from '../stores/snippetLibrary'

const props = defineProps<{ libraryId: string }>()
const emit = defineEmits<{ 'open-paper': [slug: string, page: number, title: string] }>()
const { t } = useI18n()

const searchQuery = ref('')

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
  if (!window.confirm(`删除这条素材？`)) return
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

function onTagInputKeydown(e: KeyboardEvent, s: Snippet) {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    commitTagInput()
  } else if (e.key === 'Backspace' && editTagInput.value === '' && editTagsList.value.length > 0) {
    editTagsList.value = editTagsList.value.slice(0, -1)
  } else if (e.key === 'Escape') {
    editingTagsId.value = null
  }
}

async function commitTags(s: Snippet) {
  if (editingTagsId.value !== s.id) return
  commitTagInput()
  editingTagsId.value = null
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
        <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
        </svg>
        <span class="lib-name">{{ library?.name ?? t('snippets.title') }}</span>
        <span class="item-count">{{ items.length }}</span>
      </div>
      <div class="search-wrap">
        <svg class="search-icon" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
        </svg>
        <input v-model="searchQuery" class="search-input" :placeholder="t('snippets.search')" />
      </div>
    </div>

    <!-- Empty state -->
    <div v-if="items.length === 0" class="empty-state">
      <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" opacity="0.35">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
        <line x1="12" y1="11" x2="12" y2="17"/>
        <line x1="9" y1="14" x2="15" y2="14"/>
      </svg>
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
                      placeholder="添加标签…"
                      @keydown="onTagInputKeydown($event, s)"
                      @blur="commitTags(s)"
                    />
                  </div>
                </div>
                <!-- View mode -->
                <div v-else class="tag-list editable-cell" title="点击编辑标签" @click="startEditTags(s)">
                  <span v-for="tag in s.tags" :key="tag" class="tag-chip">{{ tag }}</span>
                  <span v-if="s.tags.length === 0" class="no-tag placeholder">+ 添加</span>
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
                  title="点击编辑备注"
                  @click="startEditNote(s)"
                >{{ s.note || '+ 添加备注' }}</span>
              </td>

              <!-- Delete -->
              <td class="col-action" @click.stop>
                <button class="del-btn" :title="t('hl.delete')" @click="handleDelete(s)">
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="3 6 5 6 21 6"/>
                    <path d="M19 6l-1 14H6L5 6"/>
                    <path d="M10 11v6"/><path d="M14 11v6"/>
                    <path d="M9 6V4h6v2"/>
                  </svg>
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
</style>
