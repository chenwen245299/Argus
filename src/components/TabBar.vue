<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import { useCollectionsStore } from '../stores/collections'
import { useCanvasStore } from '../stores/canvas'
import { titleInitialCaps } from '../utils/text'

const { t } = useI18n()
const reader = useReaderStore()
const selection = useSelectionStore()
const collections = useCollectionsStore()
const canvasStore = useCanvasStore()
const isFullscreenLayout = ref(false)
const appWindow = getCurrentWebviewWindow()
let unlistenResize: UnlistenFn | null = null
let refreshTimers: number[] = []

// ── Tab drag-and-drop (pointer-based, avoids macOS native DnD green +) ────────
const dragFrom = ref<number | null>(null)
const dropAt = ref<number | null>(null)
const tabsScrollRef = ref<HTMLElement | null>(null)

function onTabMouseDown(e: MouseEvent, idx: number) {
  if (e.button !== 0) return
  if ((e.target as HTMLElement).closest('.tab-close')) return

  const startX = e.clientX
  let dragging = false

  const onMove = (ev: MouseEvent) => {
    if (!dragging) {
      if (Math.abs(ev.clientX - startX) < 5) return
      dragging = true
      dragFrom.value = idx
    }
    // Find drop position from live tab positions
    const tabs = tabsScrollRef.value?.querySelectorAll<HTMLElement>('.tab:not(.tab-home)')
    if (!tabs) return
    let di = reader.tabs.length
    tabs.forEach((el, i) => {
      const { left, width } = el.getBoundingClientRect()
      if (ev.clientX < left + width / 2 && i < di) di = i
    })
    dropAt.value = di
  }

  const onUp = () => {
    document.removeEventListener('mousemove', onMove)
    document.removeEventListener('mouseup', onUp)
    if (dragging && dragFrom.value !== null && dropAt.value !== null) {
      reader.reorderTabs(dragFrom.value, dropAt.value)
    }
    dragFrom.value = null
    dropAt.value = null
  }

  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
}

const homeTitle = computed(() => {
  if (selection.activeNav === 'recent') return t('sidebar.recentPapers')
  if (!selection.activeCollectionId) return t('sidebar.allPapers')
  return collections.collectionById(selection.activeCollectionId)?.name ?? t('sidebar.allPapers')
})

function showHome() {
  canvasStore.isShown = false
  reader.showList()
}

function showCanvas() {
  canvasStore.isShown = true
  reader.showList()
}

function closeCanvasTab() {
  void canvasStore.closeCurrentCanvas()
  reader.showList()
}

function switchTab(slug: string) {
  canvasStore.isShown = false
  reader.switchTab(slug)
}

function startDrag(e: MouseEvent) {
  if (e.button === 0) appWindow.startDragging()
}

async function refreshWindowLayout() {
  try {
    const fullscreen = await appWindow.isFullscreen()
    isFullscreenLayout.value = fullscreen
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
    <div ref="tabsScrollRef" class="tabs-scroll">
      <!-- Permanent home tab (current collection, cannot be closed) -->
      <div
        class="tab tab-home"
        :class="{ active: !reader.activeSlug && !canvasStore.isShown }"
        :title="homeTitle"
        @click="showHome()"
      >
        <svg class="tab-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <rect x="3" y="3" width="8" height="8" rx="1"/>
          <rect x="13" y="3" width="8" height="8" rx="1"/>
          <rect x="3" y="13" width="8" height="8" rx="1"/>
          <rect x="13" y="13" width="8" height="8" rx="1"/>
        </svg>
        <span class="tab-title">{{ homeTitle }}</span>
      </div>

      <!-- Canvas tab (always shown while a canvas is loaded, regardless of active state) -->
      <div
        v-if="canvasStore.currentCanvas"
        class="tab tab-canvas"
        :class="{ active: canvasStore.isShown }"
        :title="canvasStore.currentCanvas.name"
        @click="showCanvas()"
      >
        <svg class="tab-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <circle cx="8" cy="8" r="2.5"/><circle cx="16" cy="16" r="2.5"/>
          <line x1="10" y1="8" x2="14" y2="16"/>
          <circle cx="16" cy="8" r="2.5"/>
          <line x1="10" y1="9" x2="14" y2="15"/>
        </svg>
        <span class="tab-title">{{ canvasStore.currentCanvas.name }}</span>
        <button class="tab-close" @click.stop="closeCanvasTab">
          <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8">
            <line x1="18" y1="6" x2="6" y2="18"/>
            <line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      </div>

      <!-- PDF tabs -->
      <div
        v-for="(tab, idx) in reader.tabs"
        :key="tab.slug"
        class="tab"
        :class="{
          active: tab.slug === reader.activeSlug && !canvasStore.isShown,
          'tab-dragging': dragFrom === idx,
          'drop-before': dropAt === idx && dragFrom !== idx,
          'drop-after': dropAt === idx + 1 && dragFrom !== idx,
        }"
        :title="titleInitialCaps(tab.title)"
        @click="switchTab(tab.slug)"
        @mousedown="onTabMouseDown($event, idx)"
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
