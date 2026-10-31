<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import VditorEditor from '../VditorEditor.vue'
import MarkdownBody from '../MarkdownBody.vue'
import { confirm } from '@tauri-apps/plugin-dialog'
import { useLibraryStore } from '../../stores/library'
import { broadcastNoteSaved, onNoteSavedElsewhere, onNotesChanged, type NoteChangedPayload } from '../../utils/noteSync'
import { toDisplayMarkdown, toStoredMarkdown, savePastedImage } from '../../utils/noteAssets'
import type { Note, DeletedNote } from '../../types'
import type { UnlistenFn } from '@tauri-apps/api/event'

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

// The pin guide (what pinning a note buys you) is shown inline in the empty-list
// state — whenever a paper has no notes there's free space to explain it, and it
// disappears on its own once the first note exists. No popup, no dismissal.

// ── View state ────────────────────────────────────────────────────────────────
type View = 'list' | 'editor'
const view = ref<View>('list')

// ── List state ────────────────────────────────────────────────────────────────
const notes = ref<Note[]>([])
const loadingList = ref(false)
const deletedNotes = ref<DeletedNote[]>([])
const trashExpanded = ref(false)
const previewDeletedId = ref<string | null>(null)
const deletedPreview = ref('')
const operationError = ref('')
const busyNoteId = ref<string | null>(null)
const activeNoteDeleted = ref(false)
let listRevision = 0
let editorRoot: string | null = null
let editorSlug: string | null = null
let unlistenNotesChanged: UnlistenFn | null = null
let retentionTimer: ReturnType<typeof setInterval> | null = null

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
const editorRef = ref<InstanceType<typeof VditorEditor> | null>(null)
let debounceTimer: ReturnType<typeof setTimeout> | null = null
let openNoteSeq = 0
let unlistenNoteSaved: UnlistenFn | null = null

function createNoteWindowLabel() {
  const suffix = Math.random().toString(36).slice(2, 10)
  return `note-window-${Date.now()}-${suffix}`
}

// ── Remember which note was open (per paper) ────────────────────────────────
// The Notes tab is unmounted when the user switches to another right-sidebar
// tab, so persist the open note id and restore it on remount instead of dumping
// the user back on the list.
function openNoteStorageKey(slug: string) {
  return `argus:notes-open:${slug}`
}
function persistOpenNote(noteId: string | null) {
  if (!props.slug) return
  try {
    if (noteId) localStorage.setItem(openNoteStorageKey(props.slug), noteId)
    else localStorage.removeItem(openNoteStorageKey(props.slug))
  } catch {
    // storage disabled — non-fatal
  }
}
function readOpenNote(slug: string): string | null {
  try {
    return localStorage.getItem(openNoteStorageKey(slug))
  } catch {
    return null
  }
}

// ── Load note list ─────────────────────────────────────────────────────────────
async function loadList(slug: string) {
  const revision = ++listRevision, root = library.currentPath
  loadingList.value = true
  try {
    const [live, deleted] = await Promise.all([
      invoke<Note[]>('list_notes', { slug, expectedRoot: root }),
      invoke<DeletedNote[]>('list_deleted_notes', { slug, expectedRoot: root }),
    ])
    if (revision !== listRevision || props.slug !== slug || library.currentPath !== root) return
    notes.value = live
    deletedNotes.value = deleted
    if (!deleted.some(n => n.id === previewDeletedId.value)) previewDeletedId.value = null
  } catch (error) {
    if (revision === listRevision) operationError.value = String(error)
  } finally {
    if (revision === listRevision) loadingList.value = false
  }
}

function daysRemaining(note: DeletedNote) {
  return Math.max(0, Math.ceil((Date.parse(note.expires_at) - Date.now()) / 86400000))
}

async function previewDeleted(note: DeletedNote) {
  if (previewDeletedId.value === note.id) { previewDeletedId.value = null; return }
  const slug = props.slug, root = library.currentPath
  previewDeletedId.value = note.id
  deletedPreview.value = ''
  const content = slug ? await toDisplayMarkdown(slug, note.content) : note.content
  if (props.slug === slug && library.currentPath === root && previewDeletedId.value === note.id) deletedPreview.value = content
}

async function restoreNote(note: Note) {
  const slug = props.slug, root = library.currentPath
  if (!slug || busyNoteId.value) return
  busyNoteId.value = note.id
  operationError.value = ''
  try {
    await invoke('restore_note', { slug, noteId: note.id, expectedRoot: root })
    if (props.slug !== slug || library.currentPath !== root) return
    activeNoteDeleted.value = false
    await loadList(slug)
    window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', { detail: { slug } }))
    if (activeNote.value?.id === note.id) await maybeSave()
  } catch (error) {
    if (props.slug === slug && library.currentPath === root) operationError.value = String(error)
  } finally { busyNoteId.value = null }
}

// ── Open a note in the editor ─────────────────────────────────────────────────
async function openNote(note: Note) {
  const seq = ++openNoteSeq, slug = props.slug, root = library.currentPath
  // Flush any pending save first
  await maybeSave()
  if (seq !== openNoteSeq || (saveError.value && currentContent.value !== loadedContent.value)) return

  let md = ''
  try {
    const stored = await invoke<string>('get_note', { slug, noteId: note.id, expectedRoot: root })
    // Swap `assets/…` for renderable blob URLs; `flushSave` swaps them back.
    md = slug ? await toDisplayMarkdown(slug, stored) : stored
  } catch (error) {
    if (seq === openNoteSeq) operationError.value = String(error)
    return
  }
  if (seq !== openNoteSeq || props.slug !== slug || library.currentPath !== root) return

  editorRoot = root
  editorSlug = slug
  activeNoteDeleted.value = false
  activeNote.value = note
  titleDraft.value = note.title
  editingTitle.value = false
  loadedContent.value = md
  currentContent.value = md
  editorKey.value++
  view.value = 'editor'
  persistOpenNote(note.id)
}

// ── Back to list ──────────────────────────────────────────────────────────────
async function goBack() {
  if (activeNoteDeleted.value && currentContent.value !== loadedContent.value) { operationError.value = t('notes.deletedWhileEditing'); return }
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
  // Returning to the list is an explicit choice — don't reopen on remount.
  persistOpenNote(null)

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
  const slug = props.slug, root = library.currentPath
  if (!slug) return
  try {
    const note = await invoke<Note>('create_note', { slug, expectedRoot: root })
    if (props.slug !== slug || library.currentPath !== root) return
    notes.value.push(note)
    await openNote(note)
  } catch (e) {
    console.error('Failed to create note:', e)
  }
}

// ── Delete a note ─────────────────────────────────────────────────────────────
async function deleteNote(note: Note, e: MouseEvent) {
  e.stopPropagation()
  const slug = props.slug, root = library.currentPath
  if (!slug || busyNoteId.value) return
  operationError.value = ''
  busyNoteId.value = note.id
  try {
    if (!await confirm(t('notes.deleteConfirm'), { title: t('notes.deleteNote'), kind: 'warning', okLabel: t('notes.moveToTrash'), cancelLabel: t('notes.cancel') })) return
    if (props.slug !== slug || library.currentPath !== root) return
    await invoke('delete_note', { slug, noteId: note.id, expectedRoot: root })
    if (props.slug !== slug || library.currentPath !== root) return
    notes.value = notes.value.filter(n => n.id !== note.id)
    await loadList(slug)
    window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', { detail: { slug } }))
    void library.refresh()
  } catch (error) {
    if (props.slug === slug && library.currentPath === root) operationError.value = String(error)
  } finally { busyNoteId.value = null }
}

// ── Save title ────────────────────────────────────────────────────────────────
async function commitTitle() {
  editingTitle.value = false
  const note = activeNote.value, slug = editorSlug, root = editorRoot
  if (!slug || !note || activeNoteDeleted.value) return
  const trimmed = titleDraft.value.trim() || t('notes.untitled')
  if (trimmed === note.title) return
  try {
    await invoke('rename_note', { slug, noteId: note.id, title: trimmed, expectedRoot: root })
    if (editorRoot !== root || editorSlug !== slug || activeNote.value?.id !== note.id) return
    activeNote.value.title = trimmed
    window.dispatchEvent(new CustomEvent('argus-notes-updated', { detail: { slug } }))
    if (isPinned(note.id)) window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', { detail: { slug } }))
  } catch (error) {
    if (editorRoot === root && editorSlug === slug) operationError.value = String(error)
  }
}

// ── Editor save ───────────────────────────────────────────────────────────────
async function maybeSave() {
  if (!editorSlug || !activeNote.value || activeNoteDeleted.value) return
  if (currentContent.value === loadedContent.value) return
  clearTimeout(debounceTimer!)
  debounceTimer = null
  await flushSave(editorSlug, activeNote.value.id, currentContent.value)
}

async function flushSave(slug: string, noteId: string, content: string, root = editorRoot, expectedContent = toStoredMarkdown(loadedContent.value)) {
  if (activeNoteDeleted.value) return
  saving.value = true
  saveError.value = ''
  try {
    // Only relative asset paths go to disk — and to the other window, whose blob
    // URLs are its own and would be meaningless there.
    const stored = toStoredMarkdown(content)
    await invoke('save_note', { slug, noteId, content: stored, expectedRoot: root, expectedContent })
    if (editorSlug === slug && editorRoot === root && activeNote.value?.id === noteId) loadedContent.value = content
    broadcastNoteSaved(slug, noteId, stored, root)
  } catch (e) {
    saveError.value = String(e)
  } finally {
    saving.value = false
  }
}

/** Cmd+V of an image: store it under notes/assets and render it from there. */
async function uploadNoteImage(file: File): Promise<string | null> {
  if (!props.slug) return null
  return savePastedImage(props.slug, file)
}

function onContentChange(markdown: string) {
  currentContent.value = markdown
  if (!editorSlug || !activeNote.value || activeNoteDeleted.value) return
  clearTimeout(debounceTimer!)
  const slug = editorSlug, root = editorRoot
  const noteId = activeNote.value.id
  debounceTimer = setTimeout(() => flushSave(slug, noteId, markdown, root), 1500)
}

// ── Watch slug changes ────────────────────────────────────────────────────────
watch(() => [props.slug, library.currentPath] as const, async ([newSlug]) => {
  const revision = ++openNoteSeq, root = library.currentPath
  await maybeSave()
  if (revision !== openNoteSeq || props.slug !== newSlug || library.currentPath !== root) return
  activeNote.value = null
  view.value = 'list'
  notes.value = []
  deletedNotes.value = []
  trashExpanded.value = false
  operationError.value = ''
  previewDeletedId.value = null
  activeNoteDeleted.value = false
  if (newSlug) {
    await loadList(newSlug)
    if (props.slug !== newSlug || library.currentPath !== root) return
    // Restore the note that was open before the tab was switched away (or the
    // paper reopened), so the user isn't dumped back on the list every time.
    const savedId = readOpenNote(newSlug)
    if (savedId) {
      const note = notes.value.find(n => n.id === savedId)
      if (note) await openNote(note)
    }
  }
}, { immediate: true })

async function handleNotesUpdated(event: Event) {
  const detail = (event as CustomEvent<{ slug: string; openSummary?: boolean }>).detail
  const slug = detail?.slug
  if (!slug || slug !== props.slug) return
  // If the user is currently editing the AI总结 note, don't re-open it — that would
  // blow away their in-progress edits with the freshly regenerated version.
  const editingSummary = view.value === 'editor' && activeNote.value?.title === 'AI总结'
  await loadList(slug)
  // Only auto-open the AI总结 note when this event was raised by summary
  // generation completing (`openSummary`). Ordinary refreshes triggered by a
  // rename/delete must leave the user's current note/selection untouched.
  if (detail?.openSummary && !editingSummary) {
    const refreshed = notes.value.find(n => n.title === 'AI总结')
    if (refreshed) await openNote(refreshed)
  }
}

// A note edited in its standalone window autosaves there; without this the
// sidebar would keep showing the stale copy AND overwrite the window's work on
// its next flush (tab switch, unmount, opening another note).
async function handleNoteSavedElsewhere(payload: { root?: string | null; slug: string; noteId: string; content: string }) {
  if (payload.slug !== props.slug || (payload.root && payload.root !== library.currentPath) || activeNoteDeleted.value) return
  // Only worth re-reading while the list is on screen; the editor view refreshes
  // it on the way back out, and this fires on every autosave over there.
  if (view.value === 'list') void loadList(payload.slug)
  if (activeNote.value?.id !== payload.noteId) return
  // Unsaved local edits win: replacing text mid-typing is worse than letting the
  // two copies diverge until whoever saves next.
  if (currentContent.value !== loadedContent.value) return
  // The payload holds relative asset paths; resolve them for this webview.
  const display = await toDisplayMarkdown(payload.slug, payload.content)
  if (activeNote.value?.id !== payload.noteId || (payload.root && payload.root !== library.currentPath) || currentContent.value !== loadedContent.value || activeNoteDeleted.value) return
  loadedContent.value = display
  currentContent.value = display
  // A remount is the fallback when the editor hasn't finished initialising.
  if (!editorRef.value?.setContent(display)) editorKey.value++
}

async function handleNotesChanged(payload: NoteChangedPayload) {
  if (payload.root !== library.currentPath || payload.slug !== props.slug) return
  const editingId = activeNote.value?.id
  if (payload.action === 'deleted' && editingId === payload.noteId) {
    clearTimeout(debounceTimer!)
    debounceTimer = null
    activeNoteDeleted.value = true
    if (currentContent.value === loadedContent.value) {
      activeNote.value = null
      view.value = 'list'
      persistOpenNote(null)
    }
  }
  await loadList(payload.slug)
  if (payload.root !== library.currentPath || payload.slug !== props.slug || activeNote.value?.id !== payload.noteId) return
  const note = notes.value.find(n => n.id === payload.noteId)
  if (!note) return
  if (payload.action === 'restored') activeNoteDeleted.value = false
  if (currentContent.value !== loadedContent.value || editingTitle.value) return
  activeNote.value = note
  titleDraft.value = note.title
  const content = await invoke<string>('get_note', { slug: payload.slug, noteId: note.id, expectedRoot: payload.root })
  await handleNoteSavedElsewhere({ ...payload, content })
}

let noteSyncDisposed = false

onMounted(async () => {
  window.addEventListener('argus-notes-updated', handleNotesUpdated)
  const unlisten = await onNoteSavedElsewhere(handleNoteSavedElsewhere)
  // The subscription can resolve after a fast unmount — drop it right away.
  if (noteSyncDisposed) unlisten()
  else unlistenNoteSaved = unlisten
  const offChanged = await onNotesChanged(payload => { void handleNotesChanged(payload).catch(error => { operationError.value = String(error) }) })
  if (noteSyncDisposed) { offChanged(); return }
  unlistenNotesChanged = offChanged
  retentionTimer = setInterval(() => { if (props.slug && view.value === 'list') void loadList(props.slug) }, 60_000)
})

onBeforeUnmount(async () => {
  noteSyncDisposed = true
  window.removeEventListener('argus-notes-updated', handleNotesUpdated)
  unlistenNoteSaved?.()
  unlistenNotesChanged?.()
  clearInterval(retentionTimer!)
  await maybeSave()
})

// ── Open note in standalone window ────────────────────────────────────────────
async function openInWindow() {
  const note = activeNote.value, slug = editorSlug, root = editorRoot
  if (!slug || !note || activeNoteDeleted.value) return
  await maybeSave()
  if (activeNote.value?.id !== note.id || props.slug !== slug || library.currentPath !== root
    || (saveError.value && currentContent.value !== loadedContent.value)) return
  const windowLabel = createNoteWindowLabel()
  const data = { slug, noteId: note.id, title: note.title, root }
  localStorage.setItem(`argus:note-window:${windowLabel}`, JSON.stringify(data))
  try {
    await invoke('open_note_window', {
      ...data,
      expectedRoot: root,
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
    <div v-if="operationError" class="notes-error" role="alert">{{ operationError }}</div>

    <!-- No paper selected -->
    <div v-if="!slug" class="empty">{{ t('notes.selectHint') }}</div>


    <!-- Note list -->
    <template v-else-if="view === 'list'">
      <div class="list-toolbar">
        <span class="list-heading">{{ t('tabs.notes') }}</span>
        <button class="new-btn" :title="t('notes.newNote')" @click="createNote">
          <Icon icon="fluent:add-24-regular" width="15" height="15" />
        </button>
      </div>

      <div v-if="loadingList" class="empty">…</div>

      <div v-else-if="notes.length === 0" class="empty-list">
        <Icon class="empty-doodle" icon="doodle:writing-notebook" width="64" height="64" />
        <p>{{ t('notes.noNotes') }}</p>
        <span>{{ t('notes.noNotesHint') }}</span>

        <!-- Pin guide: shown here whenever the paper has no notes -->
        <div class="pin-guide-card">
          <div class="pin-guide-head">
            <span class="pin-guide-icon">
              <Icon icon="fluent:pin-24-regular" width="14" height="14" />
            </span>
            <span class="pin-guide-title">{{ t('notes.pinGuideTitle') }}</span>
          </div>
          <p class="pin-guide-intro">{{ t('notes.pinGuideIntro') }}</p>
          <ul class="pin-guide-points">
            <li>
              <span class="pin-guide-demo">
                <i class="demo-badge" style="background:#e0e7ff;color:#4338ca">A</i>
                <i class="demo-badge" style="background:#fce7f3;color:#be185d">B</i>
              </span>
              <span class="pin-guide-text">{{ t('notes.pinGuidePoint1') }}</span>
            </li>
            <li>
              <span class="pin-guide-demo"><i class="demo-thumb" /></span>
              <span class="pin-guide-text">{{ t('notes.pinGuidePoint2') }}</span>
            </li>
          </ul>
        </div>
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
                <Icon :icon="isPinned(note.id) ? 'fluent:pin-24-filled' : 'fluent:pin-24-regular'" width="11" height="11" />
              </button>
              <button
                class="note-delete-btn"
                :disabled="!!busyNoteId"
                :title="t('notes.deleteNote')"
                @click="deleteNote(note, $event)"
              >
                <Icon icon="fluent:delete-24-regular" width="13" height="13" />
              </button>
            </div>
          </div>
        </li>
      </ul>

      <section class="recently-deleted" :class="{ expanded: trashExpanded }">
        <button class="trash-toggle" type="button" :aria-expanded="trashExpanded" @click="trashExpanded = !trashExpanded">
          <Icon icon="fluent:delete-24-regular" width="15" height="15" />
          <span>{{ t('notes.recentlyDeleted') }}</span>
          <span v-if="deletedNotes.length" class="trash-count">{{ deletedNotes.length }}</span>
          <Icon class="trash-chevron" :class="{ expanded: trashExpanded }" icon="fluent:chevron-right-24-regular" width="13" height="13" />
        </button>
        <div v-if="trashExpanded" class="trash-content">
          <p class="trash-hint">{{ t('notes.trashHint') }}</p>
          <div v-if="!deletedNotes.length" class="trash-empty">{{ t('notes.trashEmpty') }}</div>
          <ul v-else class="trash-list">
            <li v-for="note in deletedNotes" :key="note.id" class="trash-item">
              <div class="trash-item-head">
                <button class="trash-note-title" type="button" :title="note.title" :aria-expanded="previewDeletedId === note.id" @click="previewDeleted(note)">
                  {{ note.title || t('notes.untitled') }}
                </button>
                <button class="restore-btn" type="button" :disabled="!!busyNoteId" :aria-label="t('notes.restoreNamed', { title: note.title })" @click="restoreNote(note)">
                  <Icon icon="fluent:arrow-counterclockwise-24-regular" width="13" height="13" />{{ t('notes.restore') }}
                </button>
              </div>
              <div class="trash-meta"><span>{{ t('notes.deletedOn', { date: fmtDate(note.deleted_at) }) }}</span><span>{{ t('notes.daysRemaining', { days: daysRemaining(note) }) }}</span></div>
              <div v-if="previewDeletedId === note.id" class="trash-preview"><MarkdownBody v-if="deletedPreview" :content="deletedPreview" /><span v-else>{{ t('notes.emptyContent') }}</span></div>
            </li>
          </ul>
        </div>
      </section>
    </template>

    <!-- Note editor -->
    <template v-else-if="view === 'editor' && activeNote">
      <div class="editor-toolbar">
        <button class="back-btn" type="button" :title="t('notes.back')" @click.stop="goBack">
          <Icon icon="fluent:chevron-left-24-regular" width="15" height="15" />
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
        <span v-else-if="saveError" class="status error" :title="saveError">{{ saveError }}</span>

        <button class="popout-btn" :title="t('notes.openInWindow')" @click="openInWindow">
          <Icon icon="fluent:open-24-regular" width="14" height="14" />
        </button>
      </div>

      <div v-if="activeNoteDeleted" class="deleted-editor-banner" role="status">
        <span>{{ t('notes.deletedWhileEditing') }}</span>
        <button class="restore-btn" :disabled="!!busyNoteId" @click="restoreNote(activeNote)">{{ t('notes.restore') }}</button>
      </div>
      <div class="editor-wrap">
        <VditorEditor
          ref="editorRef"
          :key="editorKey"
          :initial-content="loadedContent"
          :image-uploader="uploadNoteImage"
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
  position: relative;
}

.notes-error { flex-shrink: 0; padding: 8px 12px; color: var(--text-primary); background: var(--bg-secondary); font-size: 11px; border-bottom: 1px solid var(--border-subtle); overflow-wrap: anywhere; }
.recently-deleted { flex-shrink: 0; display: flex; flex-direction: column; max-height: 50%; min-height: 38px; border-top: 1px solid var(--border-subtle); background: color-mix(in srgb, var(--bg-secondary) 50%, var(--bg-primary)); }
.trash-toggle { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 38px; padding: 8px 12px; color: var(--text-secondary); font-size: 12px; text-align: left; flex-shrink: 0; }
.trash-toggle:focus-visible, .trash-note-title:focus-visible, .restore-btn:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.trash-toggle:hover { background: var(--bg-hover); color: var(--text-primary); }
.trash-count { padding: 1px 6px; border-radius: var(--radius-pill); background: var(--bg-tertiary); color: var(--text-tertiary); font-size: 10px; }
.trash-chevron { margin-left: auto; transition: transform .15s; }
.trash-chevron.expanded { transform: rotate(90deg); }
.trash-content { min-height: 0; overflow-y: auto; padding: 0 8px 8px; }
.trash-hint { padding: 0 4px 8px; margin: 0; color: var(--text-tertiary); font-size: 10px; line-height: 1.5; }
.trash-empty { padding: 16px 8px 20px; text-align: center; font-size: 12px; color: var(--text-tertiary); }
.trash-list { list-style: none; margin: 0; padding: 0; }
.trash-item { padding: 8px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); margin-bottom: 6px; }
.trash-item:last-child { margin-bottom: 0; }
.trash-item-head { display: flex; align-items: center; gap: 8px; }
.trash-note-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; text-align: left; font-size: 12px; color: var(--text-secondary); }
.trash-note-title:hover { color: var(--accent); }
.restore-btn { display: inline-flex; align-items: center; gap: 4px; flex-shrink: 0; padding: 4px 7px; font-size: 11px; color: var(--accent); background: var(--accent-light); border-radius: var(--radius-sm); }
.restore-btn:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 18%, transparent); }
.restore-btn:disabled { opacity: .5; cursor: default; }
.trash-meta { display: flex; flex-wrap: wrap; gap: 4px 10px; margin-top: 5px; font-size: 10px; color: var(--text-tertiary); }
.trash-preview { margin-top: 8px; padding-top: 8px; border-top: 1px solid var(--border-subtle); font-size: 12px; color: var(--text-secondary); overflow-wrap: anywhere; }
.deleted-editor-banner { display: flex; gap: 8px; align-items: center; padding: 8px 12px; font-size: 11px; color: var(--text-secondary); background: var(--bg-secondary); }
.note-item:focus-within .note-delete-btn, .note-item:focus-within .note-pin-btn { opacity: 1; }
/* ── First-run pin guide ── */
.pin-guide-card {
  width: 100%;
  max-width: 280px;
  margin-top: 18px;
  padding: 14px;
  text-align: left;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
}
.pin-guide-head {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-bottom: 8px;
}
.pin-guide-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  background: var(--accent-light);
  color: var(--accent);
  flex-shrink: 0;
}
.pin-guide-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}
.pin-guide-intro {
  margin: 0 0 10px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  line-height: 1.5;
}
.pin-guide-points {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.pin-guide-points li {
  display: flex;
  align-items: flex-start;
  gap: 9px;
}
.pin-guide-demo {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-top: 1px;
  width: 34px;
  justify-content: center;
}
.demo-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 15px;
  height: 15px;
  border-radius: 4px;
  font-size: 9px;
  font-weight: 700;
  font-style: normal;
}
.demo-thumb {
  width: 30px;
  height: 20px;
  border-radius: 3px;
  background: repeating-linear-gradient(
    to bottom,
    var(--text-tertiary) 0 1.5px,
    transparent 1.5px 5px
  );
  background-color: var(--bg-secondary);
  border: 1px solid var(--border-default);
  opacity: 0.8;
}
.pin-guide-text {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  line-height: 1.45;
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
  height: var(--content-header-height);
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
/* Hand-drawn figure for this panel's empty state: someone doing the thing the
   panel fills up with. Muted so it reads as an illustration, not a control. */
.empty-doodle {
  color: color-mix(in srgb, var(--accent) 55%, var(--text-tertiary));
  margin-bottom: 2px;
}

.empty-list {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: safe center;
  gap: 6px;
  padding: 24px;
  overflow-y: auto;
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
  height: var(--content-header-height);
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
.status.error { color: #cc3333; max-width: 45%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

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
  /* Pad top/bottom so superscripts/roots aren't clipped by the overflow box. */
  padding-top: 0.25em;
  padding-bottom: 0.2em;
}
</style>
