<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import { useCollectionsStore } from '../stores/collections'
import { titleInitialCaps } from '../utils/text'

const { t } = useI18n()
const reader = useReaderStore()
const selection = useSelectionStore()
const collections = useCollectionsStore()
const isFullscreenLayout = ref(false)
const appWindow = getCurrentWebviewWindow()
let unlistenResize: UnlistenFn | null = null
let refreshTimers: number[] = []

// ── Tab drag-and-drop ─────────────────────────────────────────────────────────
const dragFrom = ref<number | null>(null)
const dropAt = ref<number | null>(null) // insert-before index

function onDragStart(e: DragEvent, idx: number) {
  dragFrom.value = idx
  if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
}

function onDragOver(e: DragEvent, idx: number) {
  e.preventDefault()
  if (dragFrom.value === null) return
  const el = e.currentTarget as HTMLElement
  const { left, width } = el.getBoundingClientRect()
  dropAt.value = e.clientX < left + width / 2 ? idx : idx + 1
}

function onScrollDragOver(e: DragEvent) {
  e.preventDefault()
}

function onDrop(e: DragEvent) {
  e.preventDefault()
  if (dragFrom.value !== null && dropAt.value !== null) {
    reader.reorderTabs(dragFrom.value, dropAt.value)
  }
  dragFrom.value = null
  dropAt.value = null
}

function onDragEnd() {
  dragFrom.value = null
  dropAt.value = null
}

function onScrollDragLeave(e: DragEvent) {
  const el = e.currentTarget as HTMLElement
  if (!el.contains(e.relatedTarget as Node)) {
    dropAt.value = null
  }
}

const homeTitle = computed(() => {
  if (!selection.activeCollectionId) return t('sidebar.allPapers')
  return collections.collectionById(selection.activeCollectionId)?.name ?? t('sidebar.allPapers')
})

function startDrag(e: MouseEvent) {
  if (e.button === 0) appWindow.startDragging()
}

async function refreshWindowLayout() {
  try {
    const [fullscreen, maximized] = await Promise.all([
      appWindow.isFullscreen(),
      appWindow.isMaximized(),
    ])
    isFullscreenLayout.value = fullscreen || maximized
  } catch {
    isFullscreenLayout.value = false
  }
}

function clearRefreshTimers() {
  for (const timer of refreshTimers) window.clearTimeout(timer)
  refreshTimers = []
}

function scheduleWindowLayoutRefresh() {
  clearRefreshTimers()
  void refreshWindowLayout()
  refreshTimers = [80, 180, 360].map(delay =>
    window.setTimeout(() => {
      void refreshWindowLayout()
    }, delay)
  )
}

onMounted(async () => {
  await refreshWindowLayout()
  unlistenResize = await appWindow.onResized(scheduleWindowLayoutRefresh)
  window.addEventListener('resize', scheduleWindowLayoutRefresh)
})

onUnmounted(() => {
  clearRefreshTimers()
  unlistenResize?.()
  window.removeEventListener('resize', scheduleWindowLayoutRefresh)
})
</script>

<template>
  <div class="titlebar" :class="{ 'fullscreen-layout': isFullscreenLayout }" data-tauri-drag-region>
    <!-- Space for macOS traffic lights (~76px) — draggable -->
    <div class="tl-space" data-tauri-drag-region @mousedown="startDrag" />

    <!-- Tabs -->
    <div
      class="tabs-scroll"
      @dragover="onScrollDragOver"
      @drop="onDrop"
      @dragleave="onScrollDragLeave"
    >
      <!-- Permanent home tab (current collection, cannot be closed) -->
      <div
        class="tab tab-home"
        :class="{ active: !reader.activeSlug }"
        :title="homeTitle"
        @click="reader.showList()"
      >
        <svg class="tab-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <rect x="3" y="3" width="8" height="8" rx="1"/>
          <rect x="13" y="3" width="8" height="8" rx="1"/>
          <rect x="3" y="13" width="8" height="8" rx="1"/>
          <rect x="13" y="13" width="8" height="8" rx="1"/>
        </svg>
        <span class="tab-title">{{ homeTitle }}</span>
      </div>

      <!-- PDF tabs -->
      <div
        v-for="(tab, idx) in reader.tabs"
        :key="tab.slug"
        class="tab"
        :class="{
          active: tab.slug === reader.activeSlug,
          'tab-dragging': dragFrom === idx,
          'drop-before': dropAt === idx && dragFrom !== idx,
          'drop-after': dropAt === idx + 1 && dragFrom !== idx,
        }"
        draggable="true"
        :title="titleInitialCaps(tab.title)"
        @click="reader.switchTab(tab.slug)"
        @dragstart="onDragStart($event, idx)"
        @dragover.stop="onDragOver($event, idx)"
        @drop.stop="onDrop"
        @dragend="onDragEnd"
      >
        <svg class="tab-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
          <polyline points="14 2 14 8 20 8"/>
        </svg>
        <span class="tab-title">{{ titleInitialCaps(tab.title) }}</span>
        <button class="tab-close" @click.stop="reader.closeTab(tab.slug)">
          <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8">
            <line x1="18" y1="6" x2="6" y2="18"/>
            <line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      </div>
    </div>

    <!-- Right blank area — draggable -->
    <div class="tl-right" data-tauri-drag-region @mousedown="startDrag" />
  </div>
</template>

<style scoped>
.titlebar {
  --traffic-space: 76px;
  height: 38px;
  display: flex;
  align-items: stretch;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  -webkit-app-region: drag;
  user-select: none;
  -webkit-user-select: none;
}
.titlebar.fullscreen-layout {
  --traffic-space: 0px;
}

.tl-space {
  width: var(--traffic-space);
  flex-shrink: 0;
  -webkit-app-region: drag;
  cursor: default;
  transition: width 0.18s ease;
  will-change: width;
}

.tl-right {
  flex: 1;
  -webkit-app-region: drag;
  cursor: default;
}

.tabs-scroll {
  display: flex;
  align-items: stretch;
  overflow-x: auto;
  overflow-y: hidden;
  max-width: calc(100% - var(--traffic-space) - 60px);
  scrollbar-width: none;
  -webkit-app-region: no-drag;
  padding: 5px 3px 0;
  gap: 2px;
  transition: max-width 0.18s ease;
  will-change: max-width;
}
.tabs-scroll::-webkit-scrollbar { display: none; }

.tab {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 0 8px 0 11px;
  min-width: 100px;
  max-width: 180px;
  flex-shrink: 0;
  cursor: pointer;
  color: var(--text-tertiary);
  font-size: 12px;
  border-radius: 7px 7px 0 0;
  border: 1px solid transparent;
  border-bottom: none;
  transition: background 0.1s, color 0.1s;
  position: relative;
}
.tab:hover { background: var(--bg-hover); color: var(--text-primary); }

.tab.active {
  background: var(--bg-primary);
  color: var(--text-primary);
  font-weight: 500;
  border-color: var(--border-subtle);
  margin-bottom: -1px;
  padding-bottom: 1px;
}

.tab-home {
  min-width: 110px;
  padding-right: 12px;
}

.tab-icon {
  flex-shrink: 0;
  opacity: 0.55;
}
.tab.active .tab-icon { opacity: 1; color: var(--accent); }

.tab-title {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab-close {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  border-radius: 3px;
  color: var(--text-tertiary);
  opacity: 0;
  transition: opacity 0.1s, background 0.1s;
}
.tab:hover .tab-close,
.tab.active .tab-close { opacity: 1; }
.tab-close:hover { background: var(--bg-active); color: var(--text-primary); }

/* Drag-and-drop */
.tab-dragging {
  opacity: 0.35;
  cursor: grabbing;
}

.drop-before::before,
.drop-after::after {
  content: '';
  position: absolute;
  top: 6px;
  bottom: 6px;
  width: 2px;
  background: var(--accent);
  border-radius: 1px;
  z-index: 2;
}
.drop-before::before { left: -2px; }
.drop-after::after  { right: -2px; }

@media (prefers-reduced-motion: reduce) {
  .tl-space,
  .tabs-scroll {
    transition: none;
  }
}
</style>
