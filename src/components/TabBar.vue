<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useReaderStore, TAB_GROUP_COLORS, type Tab, type TabGroup, type TabGroupColor } from '../stores/reader'
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

// ── Tab groups ────────────────────────────────────────────────────────────────
// Paper tabs render as a flat list of segments: either a lone tab or a group
// (chip + its members, which the store keeps contiguous). Every draggable slot
// carries `data-drop-index` — its index in `reader.tabs` — so drop targets stay
// correct even when a collapsed group hides its members from the DOM.

type PaperSegment =
  | { kind: 'tab'; key: string; tab: Tab; index: number }
  | { kind: 'group'; key: string; group: TabGroup; index: number; items: { tab: Tab; index: number }[] }

const paperSegments = computed<PaperSegment[]>(() => {
  const out: PaperSegment[] = []
  reader.tabs.forEach((tab, index) => {
    const group = tab.groupId ? reader.groupById(tab.groupId) : null
    if (!group) {
      out.push({ kind: 'tab', key: tab.slug, tab, index })
      return
    }
    const last = out[out.length - 1]
    if (last?.kind === 'group' && last.group.id === group.id) {
      last.items.push({ tab, index })
      return
    }
    out.push({ kind: 'group', key: group.id, group, index, items: [{ tab, index }] })
  })
  return out
})

function groupHasActive(group: TabGroup) {
  return reader.tabs.some(t => t.groupId === group.id && t.slug === reader.activeSlug) && !canvasStore.isShown
}

// ── Tab drag-and-drop (pointer-based, avoids macOS native DnD green +) ────────
const dragFrom = ref<number | null>(null)
const dragGroupId = ref<string | null>(null)
const dropAt = ref<number | null>(null)
const tabsScrollRef = ref<HTMLElement | null>(null)

/** Index in `reader.tabs` the pointer currently sits before. */
function dropIndexAt(clientX: number): number {
  const slots = tabsScrollRef.value?.querySelectorAll<HTMLElement>('[data-drop-index]')
  let di = reader.tabs.length
  slots?.forEach(el => {
    const i = Number(el.dataset.dropIndex)
    if (!Number.isFinite(i)) return
    const { left, width } = el.getBoundingClientRect()
    if (clientX < left + width / 2 && i < di) di = i
  })
  return di
}

/**
 * Shared press-and-drag gesture. `onDrop` only fires once the pointer actually
 * moved, so a plain click still falls through to the element's own handler.
 */
function beginDrag(e: MouseEvent, onStart: () => void, onDrop: (dropIdx: number) => void) {
  const startX = e.clientX
  let dragging = false

  const onMove = (ev: MouseEvent) => {
    if (!dragging) {
      if (Math.abs(ev.clientX - startX) < 5) return
      dragging = true
      onStart()
    }
    dropAt.value = dropIndexAt(ev.clientX)
  }

  const onUp = () => {
    document.removeEventListener('mousemove', onMove)
    document.removeEventListener('mouseup', onUp)
    if (dragging && dropAt.value !== null) onDrop(dropAt.value)
    dragFrom.value = null
    dragGroupId.value = null
    dropAt.value = null
  }

  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
}

function onTabMouseDown(e: MouseEvent, idx: number) {
  if (e.button !== 0) return
  if ((e.target as HTMLElement).closest('.tab-close')) return
  beginDrag(e, () => { dragFrom.value = idx }, dropIdx => {
    if (dragFrom.value !== null) reader.reorderTabs(dragFrom.value, dropIdx)
  })
}

// `click` fires after `mouseup`, by which point the drag state is already torn
// down — so a finished drag is remembered here to stop it toggling the group.
let chipDragged = false

function onGroupChipMouseDown(e: MouseEvent, segment: PaperSegment) {
  if (e.button !== 0 || segment.kind !== 'group') return
  beginDrag(e, () => { dragGroupId.value = segment.group.id }, dropIdx => {
    if (dragGroupId.value) reader.reorderTabGroup(dragGroupId.value, dropIdx)
    chipDragged = true
  })
}

// ── Tab / group context menus ─────────────────────────────────────────────────
const tabMenu = ref<{ x: number; y: number; slug: string } | null>(null)
const groupMenu = ref<{ x: number; y: number; groupId: string } | null>(null)
const groupNameInput = ref<HTMLInputElement | null>(null)

const menuTab = computed(() => reader.tabs.find(t => t.slug === tabMenu.value?.slug) ?? null)
const menuGroup = computed(() => reader.groupById(groupMenu.value?.groupId))
/** Groups a tab can be moved into — everything except the one it's already in. */
const otherGroups = computed(() =>
  reader.tabGroups.filter(g => g.id !== menuTab.value?.groupId)
)

/** Keep a menu inside the window; both are ~210px wide. */
function menuPosition(e: MouseEvent, height: number) {
  return {
    x: Math.min(e.clientX, Math.max(8, window.innerWidth - 218)),
    y: Math.min(e.clientY, Math.max(8, window.innerHeight - height)),
  }
}

function closeMenus() {
  tabMenu.value = null
  groupMenu.value = null
}

function openTabMenu(e: MouseEvent, slug: string) {
  closeMenus()
  tabMenu.value = { ...menuPosition(e, 260), slug }
}

function openGroupMenu(e: MouseEvent, groupId: string) {
  closeMenus()
  groupMenu.value = { ...menuPosition(e, 220), groupId }
  nextTick(() => groupNameInput.value?.focus())
}

function groupTab(slug: string) {
  const id = reader.createTabGroup([slug])
  closeMenus()
  if (!id) return
  // Straight into renaming, like Chrome's "add to new group".
  nextTick(() => {
    const el = tabsScrollRef.value?.querySelector<HTMLElement>(`[data-group-chip="${id}"]`)
    const rect = el?.getBoundingClientRect()
    if (rect) {
      groupMenu.value = { x: rect.left, y: rect.bottom + 4, groupId: id }
      nextTick(() => groupNameInput.value?.select())
    }
  })
}

function moveTabToGroup(slug: string, groupId: string | null) {
  reader.setTabGroup(slug, groupId)
  closeMenus()
}

function onGroupChipClick(group: TabGroup) {
  if (chipDragged) { chipDragged = false; return }
  reader.toggleTabGroupCollapsed(group.id)
}

function applyGroupColor(color: TabGroupColor) {
  if (menuGroup.value) reader.setTabGroupColor(menuGroup.value.id, color)
}

function onDocumentMouseDown(e: MouseEvent) {
  if (!tabMenu.value && !groupMenu.value) return
  if ((e.target as HTMLElement).closest('.tabbar-menu')) return
  closeMenus()
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
  document.addEventListener('mousedown', onDocumentMouseDown)
})

onUnmounted(() => {
  clearRefreshTimers()
  unlistenResize?.()
  window.removeEventListener('resize', scheduleWindowLayoutRefresh)
  document.removeEventListener('mousedown', onDocumentMouseDown)
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

      <!-- PDF tabs, grouped Chrome-style -->
      <template v-for="segment in paperSegments" :key="segment.key">
        <!-- Ungrouped tab -->
        <div
          v-if="segment.kind === 'tab'"
          class="tab tab-paper"
          :class="{
            active: segment.tab.slug === reader.activeSlug && !canvasStore.isShown,
            'tab-dragging': dragFrom === segment.index,
            'drop-before': dropAt === segment.index && dragFrom !== segment.index,
            'drop-after': dropAt === segment.index + 1 && dragFrom !== segment.index,
          }"
          :data-drop-index="segment.index"
          :title="titleInitialCaps(segment.tab.title)"
          @click="switchTab(segment.tab.slug)"
          @mousedown="onTabMouseDown($event, segment.index)"
          @contextmenu.prevent.stop="openTabMenu($event, segment.tab.slug)"
        >
          <Icon icon="fluent:document-24-regular" class="tab-icon" width="13" height="13" />
          <span class="tab-title">{{ titleInitialCaps(segment.tab.title) }}</span>
          <button class="tab-close" @click.stop="reader.closeTab(segment.tab.slug)">
            <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
          </button>
        </div>

        <!-- Group: colour sleeve wrapping a chip and its member tabs -->
        <div
          v-else
          class="tab-group"
          :class="[`group-${segment.group.color}`, { 'group-collapsed': segment.group.collapsed }]"
        >
          <button
            class="group-chip"
            :class="{ 'chip-dragging': dragGroupId === segment.group.id }"
            :data-group-chip="segment.group.id"
            :data-drop-index="segment.group.collapsed ? segment.index : undefined"
            :title="segment.group.name || '未命名分组'"
            @click="onGroupChipClick(segment.group)"
            @mousedown="onGroupChipMouseDown($event, segment)"
            @contextmenu.prevent.stop="openGroupMenu($event, segment.group.id)"
          >
            <span v-if="segment.group.name" class="group-chip-name">{{ segment.group.name }}</span>
            <span v-else class="group-chip-dot" />
            <Transition name="chipcount">
              <span v-if="segment.group.collapsed" class="group-chip-count">{{ segment.items.length }}</span>
            </Transition>
          </button>

          <!-- Members fold in and out rather than blinking: each tab animates its
               own width, and the tabs to the right slide along with it. -->
          <TransitionGroup name="tabfold">
            <div
              v-for="item in (segment.group.collapsed ? [] : segment.items)"
              :key="item.tab.slug"
              class="tab tab-paper tab-in-group"
              :class="{
                active: item.tab.slug === reader.activeSlug && !canvasStore.isShown,
                'tab-dragging': dragFrom === item.index,
                'drop-before': dropAt === item.index && dragFrom !== item.index,
                'drop-after': dropAt === item.index + 1 && dragFrom !== item.index,
              }"
              :data-drop-index="item.index"
              :title="titleInitialCaps(item.tab.title)"
              @click="switchTab(item.tab.slug)"
              @mousedown="onTabMouseDown($event, item.index)"
              @contextmenu.prevent.stop="openTabMenu($event, item.tab.slug)"
            >
              <Icon icon="fluent:document-24-regular" class="tab-icon" width="13" height="13" />
              <span class="tab-title">{{ titleInitialCaps(item.tab.title) }}</span>
              <button class="tab-close" @click.stop="reader.closeTab(item.tab.slug)">
                <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
              </button>
            </div>
          </TransitionGroup>

          <!-- A collapsed group still shows which of its tabs is the live one -->
          <Transition name="chipcount">
            <span v-if="segment.group.collapsed && groupHasActive(segment.group)" class="group-active-dot" />
          </Transition>
        </div>
      </template>
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

    <!-- ── Tab context menu ──────────────────────────────────────────────── -->
    <Teleport to="body">
      <div
        v-if="tabMenu && menuTab"
        class="tabbar-menu"
        :style="{ left: `${tabMenu.x}px`, top: `${tabMenu.y}px` }"
      >
        <button class="tabbar-menu-item" @click="groupTab(menuTab.slug)">
          <Icon icon="fluent:tab-group-24-regular" width="14" height="14" />
          添加到新分组
        </button>
        <template v-if="otherGroups.length">
          <div class="tabbar-menu-label">添加到已有分组</div>
          <button
            v-for="g in otherGroups"
            :key="g.id"
            class="tabbar-menu-item"
            @click="moveTabToGroup(menuTab.slug, g.id)"
          >
            <span class="menu-color-dot" :class="`group-${g.color}`" />
            {{ g.name || '未命名分组' }}
          </button>
        </template>
        <button v-if="menuTab.groupId" class="tabbar-menu-item" @click="moveTabToGroup(menuTab.slug, null)">
          <Icon icon="fluent:arrow-exit-20-regular" width="14" height="14" />
          从分组中移出
        </button>
        <div class="tabbar-menu-sep" />
        <button class="tabbar-menu-item" @click="reader.closeTab(menuTab.slug); closeMenus()">
          <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
          关闭标签页
        </button>
      </div>

      <!-- ── Group context menu ─────────────────────────────────────────── -->
      <div
        v-if="groupMenu && menuGroup"
        class="tabbar-menu"
        :style="{ left: `${groupMenu.x}px`, top: `${groupMenu.y}px` }"
      >
        <input
          ref="groupNameInput"
          class="group-name-input"
          :value="menuGroup.name"
          placeholder="为分组命名"
          maxlength="40"
          @input="reader.renameTabGroup(menuGroup.id, ($event.target as HTMLInputElement).value)"
          @keydown.enter="closeMenus()"
          @keydown.escape="closeMenus()"
        />
        <div class="group-color-row">
          <button
            v-for="c in TAB_GROUP_COLORS"
            :key="c"
            class="group-color-swatch"
            :class="[`group-${c}`, { selected: menuGroup.color === c }]"
            :title="c"
            @click="applyGroupColor(c)"
          />
        </div>
        <div class="tabbar-menu-sep" />
        <button class="tabbar-menu-item" @click="reader.toggleTabGroupCollapsed(menuGroup.id); closeMenus()">
          <Icon
            :icon="menuGroup.collapsed ? 'fluent:chevron-right-24-regular' : 'fluent:chevron-down-24-regular'"
            width="14"
            height="14"
          />
          {{ menuGroup.collapsed ? '展开分组' : '折叠分组' }}
        </button>
        <button class="tabbar-menu-item" @click="reader.ungroupTabs(menuGroup.id); closeMenus()">
          <Icon icon="fluent:group-dismiss-24-regular" width="14" height="14" />
          取消分组
        </button>
        <button class="tabbar-menu-item danger" @click="reader.closeTabGroup(menuGroup.id); closeMenus()">
          <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
          关闭分组
        </button>
      </div>
    </Teleport>
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
  /* Inactive tabs used to be --text-tertiary on no fill, which reads as
     "disabled" rather than "not focused". Chrome keeps inactive tab labels
     nearly as dark as the active one and separates them with a soft fill. */
  color: var(--text-secondary);
  background: color-mix(in srgb, var(--text-primary) 4%, transparent);
  font-size: 12px;
  border-radius: 7px 7px 0 0;
  border: 1px solid transparent;
  border-bottom: none;
  transition: background 0.1s, color 0.1s;
  position: relative;
}
.tab:hover {
  background: color-mix(in srgb, var(--text-primary) 9%, transparent);
  color: var(--text-primary);
}

.tab.active {
  background: var(--bg-primary);
  color: var(--text-primary);
  font-weight: 600;
  border-color: var(--border-subtle);
  margin-bottom: -1px;
  padding-bottom: 1px;
  box-shadow: 0 -1px 5px rgba(0, 0, 0, 0.07);
}

.tab-home {
  min-width: 110px;
  padding-right: 12px;
}

.tab-icon {
  flex-shrink: 0;
  opacity: 0.75;
}
.tab.active .tab-icon { opacity: 1; color: var(--accent); }

/* ── Tab groups ─────────────────────────────────────────────────────────────
   Chrome's palette. Hues are mid-tone so they hold up on light and dark
   backgrounds, and every derived shade is a color-mix against the current
   theme's tokens rather than a hard-coded tint — so the group reads correctly
   under all 18 themes without per-theme overrides. */
.group-grey   { --group-color: #7a8290; }
.group-blue   { --group-color: #2b7de9; }
.group-red    { --group-color: #e0402f; }
.group-yellow { --group-color: #e0a112; }
.group-green  { --group-color: #1ea44b; }
.group-pink   { --group-color: #dc2b83; }
.group-purple { --group-color: #9a45e8; }
.group-cyan   { --group-color: #0e9aa7; }
.group-orange { --group-color: #f2802e; }

.tab-group {
  display: flex;
  align-items: stretch;
  padding: 0 3px;
  margin: 0 1px;
  border-radius: 9px 9px 0 0;
  background: color-mix(in srgb, var(--group-color) 11%, transparent);
  flex-shrink: 0;
  position: relative;
  transition: padding 0.22s cubic-bezier(0.32, 0.72, 0, 1);
}
/* Spacing is a margin rather than `gap` so it can collapse with the tab it
   belongs to — a gap would stay at full width until the element is removed,
   leaving a visible jump at the end of the fold. */
.tab-group > * + * { margin-left: 2px; }
.tab-group.group-collapsed { padding: 0 2px; }

.group-chip {
  display: inline-flex;
  align-items: center;
  align-self: center;
  gap: 5px;
  height: 21px;
  max-width: 140px;
  padding: 0 8px;
  border-radius: 6px;
  font-size: 11.5px;
  font-weight: 650;
  cursor: pointer;
  flex-shrink: 0;
  color: color-mix(in srgb, var(--group-color) 68%, var(--text-primary));
  background: color-mix(in srgb, var(--group-color) 22%, transparent);
  transition: background 0.1s;
}
.group-chip:hover { background: color-mix(in srgb, var(--group-color) 38%, transparent); }
.group-chip.chip-dragging { opacity: 0.4; }
.group-chip-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* A nameless group is just its colour dot, so the chip shrinks to fit it. */
.group-chip:has(.group-chip-dot) { padding: 0 5px; }
.group-chip-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--group-color);
}
.group-chip-count {
  font-variant-numeric: tabular-nums;
  font-size: 10.5px;
  padding: 0 4px;
  border-radius: 5px;
  background: color-mix(in srgb, var(--group-color) 34%, transparent);
}
/* Marks a collapsed group whose hidden tab is the one on screen. */
.group-active-dot {
  align-self: center;
  width: 5px;
  height: 5px;
  margin-left: 1px;
  border-radius: 50%;
  background: var(--group-color);
}

/* ── Collapse / expand animation ────────────────────────────────────────────
   A tab has no explicit width — it sizes to its title, clamped between
   min-width and max-width — and `width: auto` can't be transitioned. Driving
   both clamps from 0 instead makes the used width follow them, so the tab
   folds from and unfolds to its natural size without measuring anything. */
.tabfold-enter-active,
.tabfold-leave-active {
  transition:
    min-width 0.22s cubic-bezier(0.32, 0.72, 0, 1),
    max-width 0.22s cubic-bezier(0.32, 0.72, 0, 1),
    padding 0.22s cubic-bezier(0.32, 0.72, 0, 1),
    margin-left 0.22s cubic-bezier(0.32, 0.72, 0, 1),
    opacity 0.16s ease;
  overflow: hidden;
  pointer-events: none;
}
.tabfold-enter-from,
.tabfold-leave-to {
  min-width: 0;
  max-width: 0;
  padding-left: 0;
  padding-right: 0;
  margin-left: 0;
  opacity: 0;
}
/* A tab leaving is taken out of the flow's control by `position: absolute` in
   the default TransitionGroup recipe — we deliberately do NOT do that here:
   the leaving tab must keep occupying (shrinking) space so the tabs after it
   slide left instead of snapping. */

/* The count badge and the active dot only exist while collapsed, so they fade
   in on the same beat as the fold. */
.chipcount-enter-active,
.chipcount-leave-active {
  transition: opacity 0.16s ease, max-width 0.22s cubic-bezier(0.32, 0.72, 0, 1);
  overflow: hidden;
}
.chipcount-enter-from,
.chipcount-leave-to {
  opacity: 0;
  max-width: 0;
}

@media (prefers-reduced-motion: reduce) {
  .tab-group,
  .tabfold-enter-active,
  .tabfold-leave-active,
  .chipcount-enter-active,
  .chipcount-leave-active { transition: none; }
}

/* Inside a sleeve the tint comes from the group, so members drop their own
   fill and pick up the group colour on hover and when active. */
.tab-in-group { background: transparent; }
.tab-in-group:hover { background: color-mix(in srgb, var(--group-color) 20%, transparent); }
.tab-group .tab.active .tab-icon { color: var(--group-color); }

/* ── Tab / group context menus ─────────────────────────────────────────────── */
.tabbar-menu {
  position: fixed;
  z-index: 3000;
  min-width: 210px;
  padding: 5px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  font-size: 12.5px;
  user-select: none;
  -webkit-app-region: no-drag;
}
.tabbar-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 9px;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  text-align: left;
  cursor: pointer;
  transition: background 0.08s, color 0.08s;
}
.tabbar-menu-item:hover { background: var(--accent); color: #fff; }
.tabbar-menu-item.danger { color: #e53e3e; }
.tabbar-menu-item.danger:hover { background: #e53e3e; color: #fff; }
.tabbar-menu-label {
  padding: 7px 9px 3px;
  font-size: 11px;
  color: var(--text-tertiary);
}
.tabbar-menu-sep {
  height: 1px;
  background: var(--border-subtle);
  margin: 4px 0;
}
.menu-color-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--group-color);
  flex-shrink: 0;
}
.group-name-input {
  width: 100%;
  height: 28px;
  padding: 0 9px;
  margin-bottom: 7px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: 12.5px;
  user-select: text;
}
.group-name-input:focus { border-color: var(--accent); }
.group-color-row {
  display: grid;
  grid-template-columns: repeat(9, 1fr);
  gap: 5px;
  padding: 1px 2px 3px;
}
.group-color-swatch {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--group-color);
  cursor: pointer;
}
.group-color-swatch.selected {
  box-shadow: 0 0 0 2px var(--bg-primary), 0 0 0 3.5px var(--group-color);
}

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
