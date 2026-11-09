<script setup lang="ts">
import { ref, shallowRef, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { confirm } from '@tauri-apps/plugin-dialog'
import { useI18n } from 'vue-i18n'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSpeechStore } from '../stores/speech'
import { HIGHLIGHT_COLORS } from '../utils/highlightColors'
import VditorEditor from './VditorEditor.vue'
import MarkdownNoteReader from './MarkdownNoteReader.vue'
import NoteStatusBar from './NoteStatusBar.vue'
import { toDisplayMarkdown, toStoredMarkdown, savePastedImage } from '../utils/noteAssets'
import type { LibraryNoteSnapshot } from '../types'

const props = defineProps<{ slug: string }>()
const { t } = useI18n()
const library = useLibraryStore(), reader = useReaderStore()
const speech = useSpeechStore()
const root = library.currentPath
const saved = ref<LibraryNoteSnapshot | null>(null)
const editorRef = ref<InstanceType<typeof VditorEditor> | null>(null)
const annotationRef = ref<InstanceType<typeof MarkdownNoteReader> | null>(null)
const editorRoot = shallowRef<HTMLElement | null>(null)
const pendingInput = ref(false)
const title = ref(''), content = ref(''), initial = ref('')
const loaded = ref(false), saving = ref(false), error = ref(''), deleted = ref(false)
const reading = ref(false), readingMarkdown = ref('')
const editorKey = ref(0)
const color = ref<string>(HIGHLIGHT_COLORS[0].value)
const markStyle = ref<'highlight' | 'underline'>('highlight')
try { if (localStorage.getItem('argus:highlight-style') === 'underline') markStyle.value = 'underline' } catch {}
watch(markStyle, value => { try { localStorage.setItem('argus:highlight-style', value) } catch {} })
const fontSize = ref(16)
const readingAloud = computed(() => speech.isReadingPaper(props.slug))
function readAloud() {
  if (readingAloud.value) speech.stop()
  else { const text = annotationRef.value?.getText(); if (text?.trim()) void speech.read(text, { source: 'markdown', paper: props.slug }) }
}
function openRelated(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  library.openRelatedPopover(props.slug, { x: rect.right, y: rect.bottom + 4 })
}
function openCitationGraph(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
  library.openCitationGraph(props.slug, { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 })
}
const dirty = computed(() => !!saved.value && (pendingInput.value || title.value !== saved.value.title || content.value !== saved.value.content))
const statisticsRoot = computed(() => reading.value ? annotationRef.value?.getRoot() ?? null : editorRoot.value)
const saveState = computed(() => deleted.value ? 'deleted' : error.value ? 'error' : saving.value ? 'saving' : dirty.value ? 'unsaved' : 'saved')
const draftKey = `argus:library-note-draft:${encodeURIComponent(root ?? '')}:${props.slug}`
let timer: ReturnType<typeof setTimeout> | undefined
let unlisten: UnlistenFn | undefined
let disposed = false, revision = 0, blocked = false
let modeRevision = 0
// A conflicted draft must keep its original base across repeated closes/reopens.
// Rebasing it to the newly loaded disk snapshot would silently authorize a later overwrite.
let conflictBase: LibraryNoteSnapshot | null = null

function keepDraft() {
  try {
    if (dirty.value) localStorage.setItem(draftKey, JSON.stringify({ base: conflictBase ?? saved.value, title: title.value, content: content.value }))
    else localStorage.removeItem(draftKey)
  } catch { /* Editing still works when session storage is full. */ }
}

async function load(force = false) {
  if (dirty.value && !force) return
  const version = ++revision
  try {
    const note = await invoke<LibraryNoteSnapshot>('get_library_note', { slug: props.slug, expectedRoot: root })
    if (!force && loaded.value && saved.value?.id === note.id && saved.value.title === note.title && saved.value.content === note.content && saved.value.collectionName === note.collectionName) return
    let draft: { base: LibraryNoteSnapshot; title: string; content: string } | null = null
    try { draft = JSON.parse(localStorage.getItem(draftKey) ?? 'null') } catch {}
    const body = draft?.base.id === note.id ? draft.content : note.content
    const display = await toDisplayMarkdown(props.slug, body)
    if (disposed || version !== revision || (dirty.value && !force)) return
    const firstLoad = !loaded.value
    const restoredDraft = draft?.base.id === note.id && (draft.content !== note.content || draft.title !== note.title)
    saved.value = note
    pendingInput.value = false
    const tab = reader.tabs.find(tab => tab.slug === props.slug)
    if (tab) tab.title = note.title
    title.value = draft?.base.id === note.id ? draft.title : note.title
    content.value = body
    initial.value = display
    readingMarkdown.value = display
    if (firstLoad) reading.value = !!body.trim() && !restoredDraft
    editorRoot.value = null
    blocked = !!draft && (draft.base.id !== note.id || draft.base.title !== note.title || draft.base.content !== note.content)
    conflictBase = blocked ? draft!.base : null
    error.value = blocked ? t('libraryNote.conflict') : ''
    loaded.value = true; deleted.value = false; editorKey.value++
  } catch (e) { if (!disposed) error.value = String(e) }
}

async function flush() {
  clearTimeout(timer)
  captureEditor()
  if (saving.value || blocked || deleted.value || !saved.value || !dirty.value) return
  saving.value = true
  try {
    while (dirty.value && !blocked && !deleted.value) {
      const nextTitle = title.value.trim(), nextContent = content.value
      if (!nextTitle) return
      const expected: LibraryNoteSnapshot = saved.value!
      const result: LibraryNoteSnapshot = await invoke<LibraryNoteSnapshot>('save_library_note', { expected, title: nextTitle, content: nextContent, expectedRoot: root })
      saved.value = result
      if (title.value.trim() === nextTitle) title.value = result.title
      const tab = reader.tabs.find(tab => tab.slug === props.slug)
      if (tab) tab.title = result.title
      error.value = ''
      keepDraft()
    }
  } catch (e) { blocked = true; conflictBase ??= saved.value; error.value = String(e); keepDraft() }
  finally { saving.value = false }
}

function changed() {
  keepDraft(); clearTimeout(timer)
  if (!blocked && !deleted.value) timer = setTimeout(() => { void flush() }, 750)
}
function captureEditor() {
  if (!pendingInput.value) return
  const markdown = editorRef.value?.getContent()
  if (markdown !== null && markdown !== undefined) {
    content.value = toStoredMarkdown(markdown); pendingInput.value = false; keepDraft()
  }
}
function onEditorInput(event: Event) {
  if (!(event.target as HTMLElement).closest('[contenteditable="true"]')) return
  pendingInput.value = true
  // Vditor's own Markdown callback is debounced. Protect the draft immediately
  // so an AI save in this interval cannot remount the editor over typed text.
  queueMicrotask(() => { if (!disposed) { captureEditor(); changed() } })
}
function onContentChange(markdown: string) { content.value = toStoredMarkdown(markdown); pendingInput.value = false; changed() }
function onUnload() { captureEditor(); keepDraft(); void flush() }
async function reload() {
  if (dirty.value && !await confirm(t('libraryNote.reloadConfirm'), { title: t('libraryNote.reload'), kind: 'warning' })) return
  clearTimeout(timer)
  try { localStorage.removeItem(draftKey) } catch {}
  blocked = false; conflictBase = null
  await load(true)
}
async function readMode() {
  const version = ++modeRevision
  captureEditor()
  await flush()
  const source = content.value, display = await toDisplayMarkdown(props.slug, source)
  if (disposed || version !== modeRevision || content.value !== source) return
  readingMarkdown.value = display
  editorRoot.value = null
  reading.value = true
}
async function editMode() {
  const version = ++modeRevision, source = content.value
  const display = await toDisplayMarkdown(props.slug, source)
  if (disposed || version !== modeRevision || content.value !== source) return
  initial.value = display
  editorRoot.value = null
  editorKey.value++
  reading.value = false
}
watch(content, async value => {
  if (!reading.value) return
  const display = await toDisplayMarkdown(props.slug, value)
  if (!disposed && reading.value && value === content.value) readingMarkdown.value = display
})
function onFocus() {
  if (!dirty.value && !saving.value) void load()
  if (root && reader.activeSlug === props.slug) void reader.reloadFromDisk(props.slug, root)
}
function onKey(event: KeyboardEvent) {
  if (reader.activeSlug === props.slug && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
    event.preventDefault(); void flush()
  }
}
onMounted(async () => {
  unlisten = await listen<{ root: string; slug: string; action: string }>('library-notes-changed', ({ payload }) => {
    if (payload.root !== root || payload.slug !== props.slug || disposed) return
    if (payload.action === 'deleted') {
      deleted.value = true; clearTimeout(timer); keepDraft()
      if (!dirty.value) reader.closeTab(props.slug)
    } else if (!saving.value && !dirty.value) void load()
  })
  if (disposed) { unlisten(); return }
  window.addEventListener('focus', onFocus)
  window.addEventListener('keydown', onKey)
  window.addEventListener('beforeunload', onUnload)
  await load()
  if (!disposed && root && library.currentPath === root) await reader.reloadFromDisk(props.slug, root)
})
onBeforeUnmount(() => {
  if (speech.isReadingPaper(props.slug)) speech.stop()
  captureEditor(); disposed = true; revision++; clearTimeout(timer); keepDraft(); void flush()
  unlisten?.(); window.removeEventListener('focus', onFocus); window.removeEventListener('keydown', onKey); window.removeEventListener('beforeunload', onUnload)
})
defineExpose({ closeToList: () => reader.showList() })
</script>

<template>
  <section class="markdown-note" :style="{ '--md-font-size': fontSize + 'px' }" @keydown.capture="onKey">
    <header class="note-header">
      <input v-model="title" class="toolbar-title" :readonly="reading" :disabled="!loaded || deleted" :title="`${title}${saved ? '\n' + saved.collectionName + ' · Markdown' : ''}`" :aria-label="t('libraryNote.title')" maxlength="120" @input="changed" @blur="flush" />
      <button class="reader-action" :class="{ active: readingAloud }" :disabled="!loaded || !content.trim()" :aria-pressed="readingAloud" @click="readAloud">
        <Icon :icon="readingAloud ? 'fluent:stop-20-filled' : 'fluent:speaker-2-20-regular'" width="15" /><span>{{ t(readingAloud ? 'pdf.readAloudStop' : 'pdf.readAloud') }}</span>
      </button>
      <button class="reader-action" :title="t('citeGraph.buttonTitle')" @click="openCitationGraph"><Icon icon="fluent:data-sunburst-20-regular" width="15" /><span>{{ t('citeGraph.buttonLabel') }}</span></button>
      <button class="reader-action" :title="t('related.buttonTitle')" @click="openRelated"><Icon icon="fluent:link-20-regular" width="15" /><span>{{ t('related.buttonLabel') }}</span></button>
      <button v-if="loaded" class="reader-action" :class="{ active: !reading }" type="button" @click="reading ? editMode() : readMode()">
        <Icon :icon="reading ? 'fluent:edit-20-regular' : 'fluent:checkmark-20-regular'" width="15" /><span>{{ t(reading ? 'libraryNote.editBody' : 'libraryNote.finishEditing') }}</span>
      </button>
      <button v-if="!reading" class="reader-action save-note" type="button" :disabled="saving" :title="saving ? t('notes.saving') : dirty ? t('libraryNote.unsaved') : t('libraryNote.saved')" @click="flush"><Icon icon="fluent:save-20-regular" width="15" /><span>{{ t('libraryNote.save') }}</span></button>
      <div class="font-controls">
        <button :title="t('ebook.fontSmaller')" :disabled="fontSize <= 10" @click="fontSize = Math.max(10, fontSize - 1)"><Icon icon="fluent:subtract-24-regular" width="14" /></button>
        <span>{{ Math.round(fontSize / 16 * 100) }}%</span>
        <button :title="t('ebook.fontLarger')" :disabled="fontSize >= 32" @click="fontSize = Math.min(32, fontSize + 1)"><Icon icon="fluent:add-24-regular" width="14" /></button>
      </div>
      <button class="icon-btn" :title="t('ebook.search')" :aria-label="t('ebook.search')" @click="annotationRef?.openFind()"><Icon icon="fluent:search-24-regular" width="15" /></button>
      <div class="color-picker" @mousedown.prevent>
        <button v-for="c in HIGHLIGHT_COLORS" :key="c.value" class="color-dot" :class="{ active: color === c.value, underline: markStyle === 'underline' }" :style="{ '--mark-color': c.value }" :title="t(c.labelKey)" :aria-label="t(c.labelKey)" :aria-pressed="color === c.value" @click="annotationRef?.highlight(c.value)" @contextmenu.prevent="color = c.value; markStyle = markStyle === 'highlight' ? 'underline' : 'highlight'" />
      </div>
    </header>
    <div v-if="error || deleted" class="note-error">
      {{ deleted ? t('libraryNote.deleted') : error }}
      <button v-if="!deleted" type="button" @click="reload">{{ t('libraryNote.reload') }}</button>
    </div>
    <MarkdownNoteReader v-if="loaded && reading" ref="annotationRef" :slug="props.slug" :markdown="readingMarkdown" v-model:color="color" v-model:mark-style="markStyle" :font-size="fontSize" />
    <div v-else-if="loaded" class="note-editor" @input.capture="onEditorInput">
      <VditorEditor ref="editorRef" :key="editorKey" hide-toolbar :library-root="root" :initial-content="initial" :image-uploader="file => savePastedImage(props.slug, file, root)" @change="onContentChange" @ready="editorRoot = $event" />
      <MarkdownNoteReader ref="annotationRef" editing :editor-root="editorRoot" :slug="props.slug" :markdown="content" v-model:color="color" v-model:mark-style="markStyle" :font-size="fontSize" />
    </div>
    <p v-else class="note-loading">{{ t('libraryNote.loading') }}</p>
    <NoteStatusBar v-if="loaded" :markdown="content" :text-root="statisticsRoot" :active="reader.activeSlug === props.slug" :save-state="saveState" :save-error="error" />
  </section>
</template>

<style scoped>
.markdown-note { position: relative; display: flex; flex-direction: column; min-width: 0; min-height: 0; height: 100%; background: var(--bg-primary); }
.note-header { display: flex; align-items: center; gap: 8px; height: var(--content-header-height); padding: 0 10px; flex-shrink: 0; background: var(--bg-secondary); border-bottom: 1px solid var(--border-subtle); font-size: var(--font-size-sm); }
.note-header input { flex: 1; min-width: 0; height: 24px; padding: 0; border: 0; background: transparent; color: var(--text-primary); font-size: var(--font-size-sm); font-weight: 500; outline: none; text-overflow: ellipsis; }
.note-header button { display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0; color: var(--text-secondary); white-space: nowrap; }
.reader-action { gap: 4px; height: 24px; padding: 0 8px; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); font-size: var(--font-size-xs); background: var(--bg-secondary); }
.reader-action:hover,.icon-btn:hover,.font-controls button:hover { background: var(--bg-tertiary); color: var(--text-primary); }
.reader-action.active { color: var(--accent); background: var(--accent-light); border-color: var(--accent); }
.font-controls { display: flex; align-items: center; gap: 6px; flex-shrink: 0; font-size: 12px; color: var(--text-secondary); }
.font-controls button,.icon-btn { width: 26px; height: 26px; padding: 0; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); }
.font-controls span { width: 36px; text-align: center; }
.color-picker { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.note-header .color-dot { width: 15px; height: 15px; padding: 0; border: 0; border-radius: 50%; background: var(--mark-color); }
.color-dot.active { outline: 2px solid var(--text-primary); outline-offset: 2px; }
.note-header .color-dot.underline { border-radius: 0; height: 3px; margin-block: 6px; }
.note-editor :deep(.vditor-reset) { font-size: var(--md-font-size) !important; user-select: text; -webkit-user-select: text; }
.note-error button { color: var(--accent); white-space: nowrap; }
.note-loading { margin: 8px 10px; font-size: 12px; color: var(--text-tertiary); }
.note-editor { position: relative; flex: 1; min-height: 0; overflow: hidden; }
.note-error { padding: 10px 20px; color: var(--color-danger, #c33); overflow-wrap: anywhere; }
.note-error button { margin-left: 12px; }
</style>
