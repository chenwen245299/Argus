<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useLibraryStore } from './stores/library'
import { useArxivStore } from './stores/arxiv'
import { useSettingsStore } from './stores/settings'
import { checkForUpdates } from './stores/update'
import MainView from './views/MainView.vue'
import ArxivView from './views/ArxivView.vue'
import CanvasView from './views/CanvasView.vue'
import LibraryChatView from './views/LibraryChatView.vue'
import PaperAiView from './views/PaperAiView.vue'
import NoteWindowView from './views/NoteWindowView.vue'

const libraryStore = useLibraryStore()
const arxivStore = useArxivStore()
const settingsStore = useSettingsStore()

function getInitialWindowLabel() {
  try {
    return getCurrentWebviewWindow().label
  } catch {
    return 'main'
  }
}

const windowLabel = ref(getInitialWindowLabel())

onMounted(async () => {
  try {
    windowLabel.value = getCurrentWebviewWindow().label
  } catch { /* running in browser/non-tauri */ }
  try {
    const currentLibrary = await invoke<string | null>('get_current_library')
    if (currentLibrary) await settingsStore.load()
  } catch { /* no library available yet */ }
  // All windows except arXiv need the library store initialized
  if (windowLabel.value !== 'arxiv') {
    libraryStore.initialize()
  }
  // Scheduler triggers a catch-up fetch via event (only the main window handles this)
  if (windowLabel.value === 'main') {
    await listen('arxiv-fetch-due', () => { arxivStore.fetchCatchUp() })
    setTimeout(() => checkForUpdates(false), 2 * 60 * 1000)
  }
})
</script>

<template>
  <div class="app-root">
    <ArxivView v-if="windowLabel === 'arxiv'" />
    <CanvasView v-else-if="windowLabel === 'canvas'" />
    <LibraryChatView v-else-if="windowLabel === 'library-chat'" />
    <PaperAiView v-else-if="windowLabel === 'paper-ai'" />
    <NoteWindowView v-else-if="windowLabel.startsWith('note-window')" />
    <MainView v-else />
  </div>
</template>

<style>
* { box-sizing: border-box; margin: 0; padding: 0; }

.app-root {
  width: 100vw;
  height: 100vh;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-family: var(--font-sans);
  user-select: none;
  -webkit-user-select: none;
}

input,
textarea,
select,
[contenteditable='true'],
.argus-md-editor,
.argus-md-editor *,
.textLayer,
.textLayer *,
.selectable-text {
  user-select: text;
  -webkit-user-select: text;
}
</style>
