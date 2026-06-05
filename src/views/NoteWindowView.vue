<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import VditorEditor from '../components/VditorEditor.vue'

interface NoteWindowData { slug: string; noteId: string; title: string }

const slug = ref<string | null>(null)
const noteId = ref<string | null>(null)
const noteTitle = ref('笔记')
const content = ref('')
const editorKey = ref(0)
const saving = ref(false)
const saveError = ref('')
const loaded = ref(false)
const isMac = navigator.userAgent.toLowerCase().includes('macintosh')

let debounceTimer: ReturnType<typeof setTimeout> | null = null
let unlisten: UnlistenFn | null = null

async function setWindowTitle(title: string) {
  document.title = `${title} — Argus 笔记`
  try { await getCurrentWebviewWindow().setTitle(`${title} — Argus 笔记`) } catch {}
}

async function loadNote(data: NoteWindowData) {
  try {
    const md = await invoke<string>('get_note', { slug: data.slug, noteId: data.noteId })
    slug.value = data.slug
    noteId.value = data.noteId
    noteTitle.value = data.title || '笔记'
    content.value = md
    editorKey.value++
    loaded.value = true
    await setWindowTitle(noteTitle.value)
  } catch (e) {
    console.error('Failed to load note:', e)
  }
}

async function flushSave(markdown: string) {
  if (!slug.value || !noteId.value) return
  saving.value = true
  saveError.value = ''
  try {
    await invoke('save_note', { slug: slug.value, noteId: noteId.value, content: markdown })
  } catch (e) {
    saveError.value = String(e)
  } finally {
    saving.value = false
  }
}

function onContentChange(markdown: string) {
  clearTimeout(debounceTimer!)
  const s = slug.value
  const nid = noteId.value
  if (!s || !nid) return
  debounceTimer = setTimeout(() => flushSave(markdown), 1500)
}

onMounted(async () => {
  // localStorage is written by the main window before calling open_note_window
  try {
    const stored = localStorage.getItem('argus:note-window')
    if (stored) {
      const data: NoteWindowData = JSON.parse(stored)
      if (data.slug && data.noteId) await loadNote(data)
    }
  } catch {}

  // Listen for updates (switching notes into an already-open window)
  unlisten = await listen<NoteWindowData>('note-window-data', async (event) => {
    await loadNote(event.payload)
  })

})

onBeforeUnmount(() => {
  unlisten?.()
  clearTimeout(debounceTimer!)
})
</script>

<template>
  <div class="note-window" :class="{ mac: isMac }">
    <!-- macOS overlay: 28px drag region with note title -->
    <div v-if="isMac" class="drag-region" data-tauri-drag-region>
      <span class="drag-title" data-tauri-drag-region>{{ noteTitle }}</span>
    </div>

    <div class="nw-editor" v-if="loaded">
      <VditorEditor
        :key="editorKey"
        :initial-content="content"
        @change="onContentChange"
      />
    </div>
    <div v-else class="nw-empty">加载中…</div>

    <!-- Save status badge -->
    <div v-if="saving || saveError" class="nw-status-badge" :class="{ error: !!saveError }">
      {{ saveError || '保存中…' }}
    </div>
  </div>
</template>

<style scoped>
.note-window {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--bg-primary);
  position: relative;
}
.note-window.mac { padding-top: 28px; }

.drag-region {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 28px;
  z-index: 100;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  -webkit-app-region: drag;
  app-region: drag;
}

.drag-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-secondary);
  pointer-events: none;
  user-select: none;
  -webkit-user-select: none;
  max-width: 60%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nw-status-badge {
  position: absolute;
  top: 6px;
  right: 12px;
  font-size: 11px;
  color: var(--text-tertiary);
  z-index: 101;
  pointer-events: none;
}
.nw-status-badge.error { color: #cc3333; }

.nw-editor {
  flex: 1;
  min-height: 0;
  position: relative;
  overflow: hidden;
}

.nw-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  font-size: 13px;
}
</style>

<!-- Vditor chrome overrides scoped to this window's editor area -->
<style>
.nw-editor .vditor {
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  background: var(--bg-primary) !important;
  min-height: 100% !important;
}
.nw-editor .vditor-toolbar { display: none !important; }
.nw-editor .vditor-content,
.nw-editor .vditor-ir {
  background: var(--bg-primary) !important;
  border: none !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  height: 100% !important;
  padding: 0 !important;
}
.nw-editor .vditor-ir pre.vditor-reset {
  margin: 0 !important;
  padding: 14px 20px !important;
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
.nw-editor .vditor-ir h1,
.nw-editor .vditor-ir h2,
.nw-editor .vditor-ir h3,
.nw-editor .vditor-ir h4 { color: var(--text-primary) !important; font-weight: 600 !important; }
.nw-editor .vditor-ir .vditor-reset > h1::before,
.nw-editor .vditor-ir .vditor-reset > h2::before,
.nw-editor .vditor-ir .vditor-reset > h3::before,
.nw-editor .vditor-ir .vditor-reset > h4::before { content: none !important; display: none !important; }
.nw-editor .vditor-ir code:not(.hljs) {
  background: var(--bg-secondary) !important;
  border: 1px solid var(--border-subtle) !important;
  border-radius: 3px !important;
  font-family: var(--font-mono) !important;
}
.nw-editor .vditor-ir .vditor-ir__preview pre {
  margin: 8px 0 12px !important;
  padding: 12px 14px !important;
  background: var(--bg-secondary) !important;
  border: 1px solid var(--border-subtle) !important;
  border-radius: var(--radius-sm) !important;
}
.nw-editor .vditor-ir .vditor-ir__preview pre > code {
  padding: 0 !important;
  font-size: 0.95em !important;
  background: transparent !important;
  border: none !important;
  font-family: var(--font-mono) !important;
}
.nw-editor .vditor-ir blockquote {
  border-left: 3px solid var(--accent) !important;
  background: var(--bg-secondary) !important;
  color: var(--text-secondary) !important;
}
.nw-editor .vditor-ir a { color: var(--accent) !important; }
.nw-editor .vditor__tip { display: none !important; }
.nw-editor .katex-display { overflow-x: auto; overflow-y: hidden; }
</style>
