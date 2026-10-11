<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useAiStore } from '../stores/ai'
import AiTab from '../components/tabs/AiTab.vue'
import type { PaperMeta } from '../types'
import { paperAiWindowSlug } from '../utils/paperAiWindow'

const ai = useAiStore()
const slug = ref(paperAiWindowSlug(getCurrentWebviewWindow().label))
const paperTitle = ref('论文 AI')
const isMac = navigator.userAgent.toLowerCase().includes('macintosh')
let unlistenResize: UnlistenFn | null = null
let resizeTimer: ReturnType<typeof setTimeout> | null = null

async function updateWindowTitle(nextSlug: string | null) {
  if (!nextSlug) {
    paperTitle.value = '论文 AI'
    return
  }
  try {
    const meta = await invoke<PaperMeta>('get_paper_meta', { slug: nextSlug })
    paperTitle.value = meta.title || '论文 AI'
  } catch {
    paperTitle.value = '论文 AI'
  }
  const title = `${paperTitle.value} — Argus`
  document.title = title
  try {
    await getCurrentWebviewWindow().setTitle(title)
  } catch {
    // Not running inside Tauri.
  }
}

async function saveWindowSize() {
  try {
    const appWindow = getCurrentWindow()
    const [physSize, sf] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()])
    await invoke('save_paper_ai_window_size', {
      width: physSize.width / sf,
      height: physSize.height / sf,
    })
  } catch {
    // Not running inside Tauri, or the window is closing.
  }
}

onMounted(async () => {
  if (!ai.loaded) await ai.load()

  try {
    const appWindow = getCurrentWindow()
    unlistenResize = await appWindow.onResized(() => {
      if (resizeTimer) clearTimeout(resizeTimer)
      resizeTimer = setTimeout(() => {
        saveWindowSize()
      }, 500)
    })
  } catch {
    // Not running inside Tauri.
  }
})

onUnmounted(() => {
  unlistenResize?.()
  if (resizeTimer) clearTimeout(resizeTimer)
  saveWindowSize()
})

watch(slug, updateWindowTitle, { immediate: true })
</script>

<template>
  <div class="paper-ai-view" :class="{ 'mac-overlay': isMac }">
    <div v-if="isMac" class="drag-region" data-tauri-drag-region>
      <span class="drag-title" :title="paperTitle" data-tauri-drag-region>{{ paperTitle }}</span>
    </div>
    <AiTab v-if="slug" :slug="slug" standalone @open-settings="() => {}" />
    <div v-else class="loading-state">
      <div class="loading-spinner" />
      <p>加载中…</p>
    </div>
  </div>
</template>

<style scoped>
.paper-ai-view {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: #ffffff;
}
.paper-ai-view.mac-overlay {
  position: relative;
  padding-top: 28px;
}
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
  padding: 0 96px;
}
.drag-title {
  min-width: 0;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--font-size-base);
  font-weight: 500;
  line-height: 18px;
  color: var(--text-secondary);
}
.loading-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
  font-size: 13px;
}
.loading-spinner {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid var(--border-default);
  border-top-color: var(--accent);
  animation: spin .8s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }
</style>
