<script setup lang="ts">
import { ref, onMounted, defineAsyncComponent } from 'vue'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useLibraryStore } from './stores/library'
import { useArxivStore } from './stores/arxiv'
import { useSettingsStore } from './stores/settings'
import { checkForUpdates } from './stores/update'

// Async views: each window label renders exactly one view, so code-split them.
// This keeps every window from loading all other windows' code (pdfjs, vue-flow,
// vditor, llamaindex …) at startup.
const MainView = defineAsyncComponent(() => import('./views/MainView.vue'))
const ArxivView = defineAsyncComponent(() => import('./views/ArxivView.vue'))
const CanvasView = defineAsyncComponent(() => import('./views/CanvasView.vue'))
const LibraryChatView = defineAsyncComponent(() => import('./views/LibraryChatView.vue'))
const EmbeddingMapView = defineAsyncComponent(() => import('./views/EmbeddingMapView.vue'))
const PaperAiView = defineAsyncComponent(() => import('./views/PaperAiView.vue'))
const NoteWindowView = defineAsyncComponent(() => import('./views/NoteWindowView.vue'))

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
  // Settings and library init are independent — run them concurrently instead
  // of serially awaiting three IPC round-trips before first paint.
  invoke<string | null>('get_current_library')
    .then((currentLibrary) => { if (currentLibrary) return settingsStore.load() })
    .catch(() => { /* no library available yet */ })
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
    <EmbeddingMapView v-else-if="windowLabel === 'embedding-map'" />
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
