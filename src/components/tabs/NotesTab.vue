<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import VditorEditor from '../VditorEditor.vue'
import { useLibraryStore } from '../../stores/library'
import type { Note } from '../../types'

const library = useLibraryStore()

const props = defineProps<{
  slug: string | null
  canvasNotes?: string[]
}>()
const emit = defineEmits<{ 'update:canvasNotes': [noteIds: string[]] }>()
const { t } = useI18n()

function isPinned(noteId: string) {
  return (props.canvasNotes ?? []).includes(noteId)
}

function togglePin(note: { id: string }, e: MouseEvent) {
  e.stopPropagation()
  const current = props.canvasNotes ?? []
  const next = current.includes(note.id)
    ? current.filter(id => id !== note.id)
    : [...current, note.id]
  emit('update:canvasNotes', next)
}

// ── View state ────────────────────────────────────────────────────────────────
type View = 'list' | 'editor'
const view = ref<View>('list')

// ── List state ────────────────────────────────────────────────────────────────
const notes = ref<Note[]>([])
const loadingList = ref(false)

// ── Editor state ──────────────────────────────────────────────────────────────
const activeNote = ref<Note | null>(null)
const editingTitle = ref(false)
const titleDraft = ref('')
let _titleCompositionEndedAt = 0
function onTitleCompositionEnd() { _titleCompositionEndedAt = Date.now() }
function isTitleIMEActive() { return Date.now() - _titleCompositionEndedAt < 100 }
const loadedContent = ref('')
const editorKey = ref(0)
const currentContent = ref('')
const saving = ref(false)
const saveError = ref('')
let debounceTimer: ReturnType<typeof setTimeout> | null = null
let openNoteSeq = 0

function createNoteWindowLabel() {
  const suffix = Math.random().toString(36).slice(2, 10)
  return `note-window-${Date.now()}-${suffix}`
}

// ── Load note list ─────────────────────────────────────────────────────────────
async function loadList(slug: string) {
  loadingList.value = true
  try {
    notes.value = await invoke<Note[]>('list_notes', { slug })
  } catch {
    notes.value = []
  } finally {
    loadingList.value = false
  }
}

// ── Open a note in the editor ─────────────────────────────────────────────────
async function openNote(note: Note) {
  const seq = ++openNoteSeq
  // Flush any pending save first
  await maybeSave()
  if (seq !== openNoteSeq) return

  let md = ''
  try {
    md = await invoke<string>('get_note', { slug: props.slug, noteId: note.id })
  } catch {
    md = ''
  }
  if (seq !== openNoteSeq) return

  activeNote.value = note
  titleDraft.value = note.title
  editingTitle.value = false
  loadedContent.value = md
  currentContent.value = md
  editorKey.value++
  view.value = 'editor'
}

// ── Back to list ──────────────────────────────────────────────────────────────
async function goBack() {
  openNoteSeq++
  const slug = props.slug
  const note = activeNote.value
  const content = currentContent.value
  const loaded = loadedContent.value

  clearTimeout(debounceTimer!)
  debounceTimer = null
  editingTitle.value = false
  activeNote.value = null
  view.value = 'list'

  if (!slug || !note) return

  if (content !== loaded) {
    await flushSave(slug, note.id, content)
    if (saveError.value) {
      activeNote.value = note
      loadedContent.value = loaded
      currentContent.value = content
      view.value = 'editor'
      editorKey.value++
      return
    }
  }

  await loadList(slug)
}

// ── Create a new note ─────────────────────────────────────────────────────────
async function createNote() {
  if (!props.slug) return
  try {
    const note = await invoke<Note>('create_note', { slug: props.slug })
    notes.value.push(note)
    await openNote(note)
  } catch (e) {
    console.error('Failed to create note:', e)
  }
}

// ── Delete a note ─────────────────────────────────────────────────────────────
async function deleteNote(note: Note, e: MouseEvent) {
  e.stopPropagation()
  if (!props.slug) return
  if (!confirm(t('notes.deleteConfirm'))) return
  try {
    const wasCanvasNote = isPinned(note.id)
    await invoke('delete_note', { slug: props.slug, noteId: note.id })
    notes.value = notes.value.filter(n => n.id !== note.id)
    window.dispatchEvent(new CustomEvent('argus-notes-updated', { detail: { slug: props.slug } }))
    if (wasCanvasNote) {
      window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', { detail: { slug: props.slug } }))
    }
    library.refresh()
    if (activeNote.value?.id === note.id) {
      activeNote.value = null
      view.value = 'list'
    }
  } catch (e) {
    console.error('Failed to delete note:', e)
  }
}

// ── Save title ────────────────────────────────────────────────────────────────
async function commitTitle() {
  editingTitle.value = false
  if (!props.slug || !activeNote.value) return
  const trimmed = titleDraft.value.trim() || t('notes.untitled')
  if (trimmed === activeNote.value.title) return
  activeNote.value.title = trimmed
  try {
    await invoke('rename_note', { slug: props.slug, noteId: activeNote.value.id, title: trimmed })
    window.dispatchEvent(new CustomEvent('argus-notes-updated', { detail: { slug: props.slug } }))
    if (isPinned(activeNote.value.id)) {
      window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', { detail: { slug: props.slug } }))
    }
  } catch (e) {
    console.error('Failed to rename note:', e)
  }
}

// ── Editor save ───────────────────────────────────────────────────────────────
async function maybeSave() {
  if (!props.slug || !activeNote.value) return
  if (currentContent.value === loadedContent.value) return
  clearTimeout(debounceTimer!)
  debounceTimer = null
  await flushSave(props.slug, activeNote.value.id, currentContent.value)
}

async function flushSave(slug: string, noteId: string, content: string) {
  saving.value = true
  saveError.value = ''
  try {
    await invoke('save_note', { slug, noteId, content })
    loadedContent.value = content
  } catch (e) {
    saveError.value = String(e)
  } finally {
    saving.value = false
  }
}

function onContentChange(markdown: string) {
  currentContent.value = markdown
  if (!props.slug || !activeNote.value) return
  clearTimeout(debounceTimer!)
  const slug = props.slug
  const noteId = activeNote.value.id
  debounceTimer = setTimeout(() => flushSave(slug, noteId, markdown), 1500)
}

// ── Watch slug changes ────────────────────────────────────────────────────────
watch(() => props.slug, async (newSlug) => {
  openNoteSeq++
  await maybeSave()
  activeNote.value = null
  view.value = 'list'
  notes.value = []
  if (newSlug) await loadList(newSlug)
}, { immediate: true })

async function handleNotesUpdated(event: Event) {
  const slug = (event as CustomEvent<{ slug: string }>).detail?.slug
  if (!slug || slug !== props.slug) return
  // If the user is currently editing the AI总结 note, don't re-open it — that would
  // blow away their in-progress edits with the freshly regenerated version. Only
  // auto-open the refreshed summary when they're on the list (or editing something else).
  const editingSummary = view.value === 'editor' && activeNote.value?.title === 'AI总结'
  await loadList(slug)
  if (!editingSummary) {
    const refreshed = notes.value.find(n => n.title === 'AI总结')
    if (refreshed) await openNote(refreshed)
  }
}

onMounted(() => {
  window.addEventListener('argus-notes-updated', handleNotesUpdated)
})

onBeforeUnmount(async () => {
  window.removeEventListener('argus-notes-updated', handleNotesUpdated)
  await maybeSave()
})

// ── Open note in standalone window ────────────────────────────────────────────
async function openInWindow() {
  if (!props.slug || !activeNote.value) return
  await maybeSave()
  const windowLabel = createNoteWindowLabel()
  const data = { slug: props.slug, noteId: activeNote.value.id, title: activeNote.value.title }
  localStorage.setItem(`argus:note-window:${windowLabel}`, JSON.stringify(data))
  try {
    await invoke('open_note_window', {
      ...data,
      windowLabel,
    })
  } catch (e) {
    localStorage.removeItem(`argus:note-window:${windowLabel}`)
    console.error('Failed to open note window:', e)
  }
}

// ── Format date ───────────────────────────────────────────────────────────────
function fmtDate(iso: string) {
  return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })
}
</script>

<template>
  <div class="notes-tab">
    <!-- No paper selected -->
    <div v-if="!slug" class="empty">{{ t('notes.selectHint') }}</div>

    <!-- Note list -->
    <template v-else-if="view === 'list'">
      <div class="list-toolbar">
        <span class="list-heading">{{ t('tabs.notes') }}</span>
        <button class="new-btn" :title="t('notes.newNote')" @click="createNote">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
          </svg>
        </button>
      </div>

      <div v-if="loadingList" class="empty">…</div>

      <div v-else-if="notes.length === 0" class="empty-list">
        <p>{{ t('notes.noNotes') }}</p>
        <span>{{ t('notes.noNotesHint') }}</span>
      </div>

      <ul v-else class="note-list">
        <li
          v-for="note in notes"
          :key="note.id"
          class="note-item"
          @click="openNote(note)"
        >
          <div class="note-item-title">{{ note.title || t('notes.untitled') }}</div>
          <div class="note-item-footer">
            <span class="note-date">{{ fmtDate(note.updated_at) }}</span>
            <div class="note-actions">
              <button
                class="note-pin-btn"
                :class="{ pinned: isPinned(note.id) }"
                :title="isPinned(note.id) ? t('notes.unpinFromCanvas') : t('notes.pinToCanvas')"
                @click="togglePin(note, $event)"
              >
                <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" stroke="none">
                  <path d="M16 1l-1.5 1.5L16 4l-6 6-2-1L6 11l3 3-5 5h2l4-4 3 3 2-2-1-2 6-6 1.5 1.5L23 8z"/>
                </svg>
              </button>
              <button
                class="note-delete-btn"
                :title="t('notes.deleteNote')"
                @click="deleteNote(note, $event)"
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <polyline points="3 6 5 6 21 6"/>
                  <path d="M19 6l-1 14H6L5 6"/>
                  <path d="M10 11v6M14 11v6"/>
                  <path d="M9 6V4h6v2"/>
                </svg>
              </button>
            </div>
          </div>
        </li>
      </ul>
    </template>

    <!-- Note editor -->
    <template v-else-if="view === 'editor' && activeNote">
      <div class="editor-toolbar">
        <button class="back-btn" type="button" :title="t('notes.back')" @click.stop="goBack">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
            <polyline points="15 18 9 12 15 6"/>
          </svg>
        </button>

        <!-- Inline editable title -->
        <input
          v-if="editingTitle"
          class="title-input"
          v-model="titleDraft"
          :placeholder="t('notes.titlePlaceholder')"
          @compositionend="onTitleCompositionEnd"
          @blur="commitTitle"
          @keydown.enter="() => { if (!isTitleIMEActive()) commitTitle() }"
          @keydown.esc="editingTitle = false; titleDraft = activeNote.title"
          ref="titleInputRef"
        />
        <span
          v-else
          class="editor-title"
          @click="editingTitle = true; nextTick(() => ($refs.titleInputRef as HTMLInputElement)?.focus())"
          :title="activeNote.title"
        >{{ activeNote.title || t('notes.untitled') }}</span>

        <span v-if="saving" class="status">{{ t('notes.saving') }}</span>
        <span v-else-if="saveError" class="status error">{{ saveError }}</span>

        <button class="popout-btn" :title="t('notes.openInWindow')" @click="openInWindow">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
          </svg>
        </button>
      </div>

      <div class="editor-wrap">
        <VditorEditor
          :key="editorKey"
          :initial-content="loadedContent"
          @change="onContentChange"
        />
      </div>
    </template>
  </div>
</template>

<style scoped>
.notes-tab {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

.empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}

/* ── List toolbar ── */
.list-toolbar {
  display: flex;
  align-items: center;
  height: 40px;
  padding: 0 10px 0 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  gap: 6px;
}

.list-heading {
  flex: 1;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.new-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--accent);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  transition: background 0.1s;
}
.new-btn:hover { background: var(--bg-tertiary); }

/* ── Empty list ── */
.empty-list {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 24px;
  color: var(--text-tertiary);
}
.empty-list p { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-secondary); margin: 0; }
.empty-list span { font-size: var(--font-size-xs); text-align: center; }

/* ── Note list ── */
.note-list {
  list-style: none;
  overflow-y: auto;
  flex: 1;
  padding: 4px 0;
}

.note-item {
  padding: 8px 12px;
  cursor: pointer;
  border-bottom: 1px solid var(--border-subtle);
  transition: background 0.1s;
}
.note-item:hover { background: var(--bg-hover); }
.note-item:hover .note-delete-btn { opacity: 1; }
.note-item:hover .note-pin-btn:not(.pinned) { opacity: 1; }

.note-item-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  margin-bottom: 3px;
}

.note-item-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.note-actions {
  display: flex;
  align-items: center;
  gap: 2px;
}

.note-date {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.note-pin-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  opacity: 0;
  transition: opacity 0.15s, color 0.1s, background 0.1s;
}
.note-pin-btn.pinned {
  opacity: 1;
  color: var(--accent);
}
.note-pin-btn:hover { color: var(--accent); background: var(--bg-hover); }
.note-item:hover .note-pin-btn { opacity: 1; }

.note-delete-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  opacity: 0;
  transition: opacity 0.15s, color 0.1s, background 0.1s;
}
.note-delete-btn:hover { color: #cc3333; background: #fff0f0; }

/* ── Editor toolbar ── */
.editor-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  flex-shrink: 0;
  height: 40px;
  position: relative;
  z-index: 2;
}

.back-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--accent);
  flex-shrink: 0;
  transition: background 0.1s;
}
.back-btn:hover { background: var(--bg-hover); }

.editor-title {
  flex: 1;
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: text;
  min-width: 0;
}
.editor-title:hover { color: var(--accent); }

.title-input {
  flex: 1;
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  padding: 2px 6px;
  outline: none;
  min-width: 0;
}

.status { font-size: var(--font-size-xs); color: var(--text-tertiary); flex-shrink: 0; }
.status.error { color: #cc3333; }

.popout-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  flex-shrink: 0;
  transition: background 0.1s, color 0.1s;
}
.popout-btn:hover { background: var(--bg-hover); color: var(--accent); }

/* ── Editor ── */
.editor-wrap {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow: hidden;
  background: var(--bg-primary);
}
</style>

<!-- Global Vditor overrides to match the app theme -->
<style>
/* Strip Vditor's default chrome inside the notes panel */
.editor-wrap .vditor {
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  background: var(--bg-primary) !important;
  min-height: 100% !important;
}

.editor-wrap .vditor-toolbar { display: none !important; }

.editor-wrap .vditor-content,
.editor-wrap .vditor-ir {
  background: var(--bg-primary) !important;
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  height: 100% !important;
  padding: 0 !important;
}

.editor-wrap .vditor-ir pre.vditor-reset {
  margin: 0 !important;
  padding: 14px 16px !important;
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  box-sizing: border-box;
  font-size: 16px !important;
  line-height: 1.7 !important;
  color: var(--text-primary) !important;
  background: var(--bg-primary) !important;
  font-family: var(--font-sans) !important;
  caret-color: var(--accent);
  min-height: 100% !important;
}

/* Headings */
.editor-wrap .vditor-ir h1,
.editor-wrap .vditor-ir h2,
.editor-wrap .vditor-ir h3,
.editor-wrap .vditor-ir h4 {
  color: var(--text-primary) !important;
  font-weight: 600 !important;
}

.editor-wrap .vditor-ir .vditor-reset > h1::before,
.editor-wrap .vditor-ir .vditor-reset > h2::before,
.editor-wrap .vditor-ir .vditor-reset > h3::before,
.editor-wrap .vditor-ir .vditor-reset > h4::before,
.editor-wrap .vditor-ir .vditor-reset > h5::before,
.editor-wrap .vditor-ir .vditor-reset > h6::before {
  content: none !important;
  display: none !important;
}

/* Inline code */
.editor-wrap .vditor-ir code:not(.hljs) {
  background: var(--bg-secondary) !important;
  border: 1px solid var(--border-subtle) !important;
  border-radius: 3px !important;
  font-family: var(--font-mono) !important;
}

/* Code blocks */
.editor-wrap .vditor-ir .vditor-ir__preview[data-render] {
  min-height: 0 !important;
}

.editor-wrap .vditor-ir .vditor-ir__preview pre {
  margin: 8px 0 12px !important;
  padding: 12px 14px !important;
  background: var(--bg-secondary) !important;
  border: 1px solid var(--border-subtle) !important;
  border-radius: var(--radius-sm) !important;
  box-sizing: border-box;
}

.editor-wrap .vditor-ir .vditor-ir__preview pre > code {
  padding: 0 !important;
  font-size: 0.95em !important;
  line-height: 1.55 !important;
  background: transparent !important;
  border: none !important;
  border-radius: 0 !important;
  font-family: var(--font-mono) !important;
}

/* Blockquote */
.editor-wrap .vditor-ir blockquote {
  border-left: 3px solid var(--accent) !important;
  background: var(--bg-secondary) !important;
  color: var(--text-secondary) !important;
}

/* Links */
.editor-wrap .vditor-ir a {
  color: var(--accent) !important;
}

/* Hide bottom info bar */
.editor-wrap .vditor__tip { display: none !important; }

/* Math blocks: full width scroll */
.editor-wrap .vditor-ir .katex-display,
.notes-preview .katex-display {
  overflow-x: auto;
  overflow-y: hidden;
}
</style>
