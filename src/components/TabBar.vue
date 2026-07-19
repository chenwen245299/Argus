<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
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

type SnippetLibraryTab = {
  id: string
  name: string
  emoji?: string
}

type WritingTab = {
  id: string | null   // null = the "Library"/all-papers view
  name: string
}

const props = defineProps<{
  rightSidebarOpen?: boolean
  snippetLibraryTabs?: SnippetLibraryTab[]
  snippetLibraryVisible?: boolean
  activeSnippetLibraryId?: string | null
  writingTabs?: WritingTab[]
  writingVisible?: boolean
  activeWritingId?: string | null
}>()
const emit = defineEmits<{
  'toggle-right-sidebar': []
  'show-home': []
  'show-canvas': []
  'switch-snippet-library': [libraryId: string]
  'close-snippet-library-tab': [libraryId: string]
  'switch-writing': [id: string | null]
  'close-writing-tab': [id: string | null]
}>()

const isFullscreenLayout = ref(false)
const isMaximized = ref(false)
const appWindow = getCurrentWebviewWindow()
const isWindows = navigator.userAgent.toLowerCase().includes('windows')
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
    const tabs = tabsScrollRef.value?.querySelectorAll<HTMLElement>('.tab.tab-paper')
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
  emit('show-home')
}

function showCanvas() {
  canvasStore.isShown = true
  reader.showList()
  emit('show-canvas')
}

function closeCanvasTab() {
  void canvasStore.closeCurrentCanvas()
  reader.showList()
}

function switchTab(slug: string) {
  canvasStore.isShown = false
  reader.switchTab(slug)
}

function switchSnippetLibrary(libraryId: string) {
  canvasStore.isShown = false
  reader.showList()
  emit('switch-snippet-library', libraryId)
}

function switchWriting(id: string | null) {
  canvasStore.isShown = false
  reader.showList()
  emit('switch-writing', id)
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

  if (!isWindows) return
  try {
    isMaximized.value = await appWindow.isMaximized()
  } catch {
    isMaximized.value = false
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

async function minimizeWindow() {
  await appWindow.minimize().catch(() => {})
}

async function toggleMaximizeWindow() {
  await appWindow.toggleMaximize().catch(() => {})
  scheduleWindowLayoutRefresh()
}

async function closeWindow() {
  await appWindow.close().catch(() => {})
}
</script>

<template>
  <div
    class="titlebar"
    :class="{ 'fullscreen-layout': isFullscreenLayout, 'windows-layout': isWindows }"
    data-tauri-drag-region
  >
    <!-- Space for macOS traffic lights (~76px) — draggable -->
    <div class="tl-space" data-tauri-drag-region @mousedown="startDrag" />

    <!-- Tabs -->
    <div ref="tabsScrollRef" class="tabs-scroll">
      <!-- Permanent home tab (current collection, cannot be closed) -->
      <div
        class="tab tab-home"
        :class="{ active: !reader.activeSlug && !canvasStore.isShown && !props.snippetLibraryVisible && !props.writingVisible }"
        :title="homeTitle"
        @click="showHome()"
      >
        <Icon icon="fluent:grid-24-regular" class="tab-icon" width="13" height="13" />
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
        <Icon icon="fluent:share-android-24-regular" class="tab-icon" width="13" height="13" />
        <span class="tab-title">{{ canvasStore.currentCanvas.name }}</span>
        <button class="tab-close" @click.stop="closeCanvasTab">
          <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
        </button>
      </div>

      <!-- Snippet library tabs -->
      <div
        v-for="tab in props.snippetLibraryTabs ?? []"
        :key="`snippet:${tab.id}`"
        class="tab tab-snippet"
        :class="{ active: props.snippetLibraryVisible && props.activeSnippetLibraryId === tab.id && !reader.activeSlug && !canvasStore.isShown }"
        :title="tab.name"
        @click="switchSnippetLibrary(tab.id)"
      >
        <span v-if="tab.emoji" class="snippet-tab-emoji">{{ tab.emoji }}</span>
        <Icon v-else icon="fluent:folder-24-regular" class="tab-icon" width="13" height="13" />
        <span class="tab-title">{{ tab.name }}</span>
        <button class="tab-close" @click.stop="emit('close-snippet-library-tab', tab.id)">
          <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
        </button>
      </div>

      <!-- Writing workspace tabs (one per open reference list; null = Library) -->
      <div
        v-for="tab in props.writingTabs ?? []"
        :key="`writing:${tab.id ?? '__all__'}`"
        class="tab tab-writing"
        :class="{ active: props.writingVisible && props.activeWritingId === tab.id && !reader.activeSlug && !canvasStore.isShown }"
        :title="tab.name"
        @click="switchWriting(tab.id)"
      >
        <Icon :icon="tab.id === null ? 'fluent:grid-24-regular' : 'fluent:document-text-24-regular'" class="tab-icon" width="13" height="13" />
        <span class="tab-title">{{ tab.name }}</span>
        <button class="tab-close" @click.stop="emit('close-writing-tab', tab.id)">
          <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
        </button>
      </div>

      <!-- PDF tabs -->
      <div
        v-for="(tab, idx) in reader.tabs"
        :key="tab.slug"
        class="tab tab-paper"
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
        <Icon icon="fluent:document-24-regular" class="tab-icon" width="13" height="13" />
        <span class="tab-title">{{ titleInitialCaps(tab.title) }}</span>
        <button class="tab-close" @click.stop="reader.closeTab(tab.slug)">
          <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
        </button>
      </div>
    </div>

    <!-- Right area — draggable filler + right-sidebar toggle -->
    <div class="tl-right" data-tauri-drag-region @mousedown="startDrag">
      <button
        class="titlebar-toggle-btn"
        :class="{ active: props.rightSidebarOpen }"
        :title="props.rightSidebarOpen ? t('pdf.hideSidebar') : t('pdf.showSidebar')"
        @mousedown.stop
        @click="emit('toggle-right-sidebar')"
      >
        <Icon icon="fluent:panel-right-24-regular" width="19" height="19" />
      </button>

      <div v-if="isWindows" class="window-controls" @mousedown.stop>
        <button class="window-control-btn" title="最小化" @click="minimizeWindow">
          <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
            <path d="M1.5 5.5h8" />
          </svg>
        </button>
        <button class="window-control-btn" :title="isMaximized ? '还原' : '最大化'" @click="toggleMaximizeWindow">
          <svg v-if="isMaximized" width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
            <path d="M3.5 1.5h6v6h-6z" />
            <path d="M1.5 3.5v6h6" />
          </svg>
          <svg v-else width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
            <path d="M1.5 1.5h8v8h-8z" />
          </svg>
        </button>
        <button class="window-control-btn close" title="关闭" @click="closeWindow">
          <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
            <path d="M2 2l7 7M9 2L2 9" />
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.titlebar {
  --traffic-space: 76px;
  --right-controls-space: 60px;
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
.titlebar.windows-layout {
  --traffic-space: 0px;
  --right-controls-space: 174px;
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
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  padding-right: 10px;
  -webkit-app-region: drag;
  cursor: default;
}
.titlebar.windows-layout .tl-right {
  padding-right: 0;
}

.titlebar-toggle-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 26px;
  border-radius: var(--radius-md);
  color: var(--text-tertiary);
  cursor: pointer;
  -webkit-app-region: no-drag;
  transition: background 0.12s, color 0.12s;
}
.titlebar-toggle-btn:hover { background: var(--bg-hover); color: var(--text-secondary); }
.titlebar-toggle-btn.active { color: var(--accent); }

.window-controls {
  align-self: stretch;
  display: flex;
  align-items: stretch;
  -webkit-app-region: no-drag;
}

.window-control-btn {
  width: 46px;
  height: 38px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.window-control-btn svg {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.2;
  vector-effect: non-scaling-stroke;
}
.window-control-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.window-control-btn.close:hover {
  background: #e81123;
  color: white;
}

.tabs-scroll {
  display: flex;
  align-items: stretch;
  overflow-x: auto;
  overflow-y: hidden;
  max-width: calc(100% - var(--traffic-space) - var(--right-controls-space));
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

.snippet-tab-emoji {
  flex-shrink: 0;
  width: 12px;
  height: 12px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  line-height: 1;
}

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
