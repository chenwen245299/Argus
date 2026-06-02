<script setup lang="ts">
import { computed } from 'vue'
import { useSelectionStore } from '../stores/selection'
import { useCollectionsStore } from '../stores/collections'
import type { Collection } from '../types'

const props = defineProps<{
  collection: Collection
  depth: number
  expanded: Set<string>
  renamingId: string | null
  renameValue: string
  dragOverId: string | null
  showNewInput: boolean
  newCollParent: string | undefined
  newCollName: string
}>()

const emit = defineEmits<{
  toggleExpand: [id: string]
  openCtx: [e: MouseEvent, col: Collection]
  startRename: [col: Collection]
  submitRename: [id: string]
  delete: [col: Collection]
  startNew: [parentId: string]
  submitNew: []
  'update:renameValue': [val: string]
  'update:newCollName': [val: string]
  'update:showNewInput': [val: boolean]
}>()

const selection = useSelectionStore()
const cStore = useCollectionsStore()

const children = computed(() => cStore.childrenOf(props.collection.id))
const isExpanded = computed(() => props.expanded.has(props.collection.id))
const isActive = computed(() => selection.activeCollectionId === props.collection.id)
const isDragOver = computed(() => props.dragOverId === props.collection.id)
const paperCount = computed(() => cStore.collectionPaperCount(props.collection.id))
const displayEmoji = computed(() => props.collection.emoji?.trim() || '📚')

function selectCollection() {
  selection.selectNav(`collection:${props.collection.id}` as any)
}
</script>

<template>
  <div class="coll-node">
    <div
      class="nav-item coll-item"
      :class="{ active: isActive, 'drag-over': isDragOver }"
      :style="{ paddingLeft: (9 + depth * 16) + 'px' }"
      :data-collection-id="collection.id"
      @click="selectCollection"
      @contextmenu.prevent="$emit('openCtx', $event, collection)"
    >
      <span class="collection-emoji" aria-hidden="true">{{ displayEmoji }}</span>

      <!-- Rename input or label -->
      <template v-if="renamingId === collection.id">
        <input
          :id="'rename-' + collection.id"
          :value="renameValue"
          class="coll-name-input inline"
          @input="$emit('update:renameValue', ($event.target as HTMLInputElement).value)"
          @keydown.enter="$emit('submitRename', collection.id)"
          @keydown.escape="$emit('update:renameValue', collection.name)"
          @blur="$emit('submitRename', collection.id)"
          @click.stop
        />
      </template>
      <template v-else>
        <span class="coll-name">{{ collection.name }}</span>
      </template>

      <!-- Expand toggle if has children -->
      <button
        v-if="children.length"
        class="expand-btn"
        @click.stop="$emit('toggleExpand', collection.id)"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"
          :style="{ transform: isExpanded ? 'rotate(90deg)' : 'rotate(0deg)', transition: 'transform 0.15s' }">
          <polyline points="9 18 15 12 9 6"/>
        </svg>
      </button>
      <span v-else class="expand-placeholder" />

      <span class="badge">{{ paperCount }}</span>
    </div>

    <!-- New sub-collection input -->
    <div
      v-if="showNewInput && newCollParent === collection.id"
      class="new-coll-row"
      :style="{ paddingLeft: (15 + (depth + 1) * 16) + 'px' }"
    >
      <input
        id="new-coll-input"
        :value="newCollName"
        class="coll-name-input"
        placeholder="Collection name"
        @input="$emit('update:newCollName', ($event.target as HTMLInputElement).value)"
        @keydown.enter="$emit('submitNew')"
        @keydown.escape="$emit('update:showNewInput', false)"
        @blur="$emit('submitNew')"
      />
    </div>

    <!-- Children (recursive) -->
    <template v-if="isExpanded && children.length">
      <CollectionNode
        v-for="child in children"
        :key="child.id"
        :collection="child"
        :depth="depth + 1"
        :expanded="expanded"
        :renamingId="renamingId"
        :renameValue="renameValue"
        :dragOverId="dragOverId"
        :showNewInput="showNewInput"
        :newCollParent="newCollParent"
        :newCollName="newCollName"
        @toggle-expand="(id: string) => $emit('toggleExpand', id)"
        @open-ctx="(e: MouseEvent, col: Collection) => $emit('openCtx', e, col)"
        @start-rename="(col: Collection) => $emit('startRename', col)"
        @submit-rename="(id: string) => $emit('submitRename', id)"
        @delete="(col: Collection) => $emit('delete', col)"
        @start-new="(id: string) => $emit('startNew', id)"
        @submit-new="() => $emit('submitNew')"
        @update:renameValue="(v: string) => $emit('update:renameValue', v)"
        @update:newCollName="(v: string) => $emit('update:newCollName', v)"
        @update:showNewInput="(v: boolean) => $emit('update:showNewInput', v)"
      />
    </template>
  </div>
</template>

<style scoped>
.coll-node { display: flex; flex-direction: column; }

.nav-item {
  display: flex;
  align-items: center;
  gap: 7px;
  width: calc(100% - 12px);
  margin: 1px 6px;
  padding: 5px 9px;
  font-size: 13px;
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  text-align: left;
  transition: background 0.12s, color 0.12s, outline-color 0.12s;
  white-space: nowrap;
  overflow: hidden;
  cursor: pointer;
  user-select: none;
  -webkit-user-select: none;
}
.nav-item:hover { background: var(--bg-hover); color: var(--text-primary); }
.nav-item.active { background: var(--bg-active); color: var(--accent); font-weight: 500; }
.nav-item.drag-over {
  background: color-mix(in srgb, var(--accent) 18%, transparent);
  outline: 1.5px dashed var(--accent);
  outline-offset: -2px;
  color: var(--accent);
}

.collection-emoji {
  width: 15px;
  height: 17px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  line-height: 1;
}

.expand-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  color: var(--text-tertiary);
  border-radius: 2px;
}
.expand-btn:hover { background: var(--bg-hover); color: var(--text-secondary); }
.nav-item.active .expand-btn { color: currentColor; }
.expand-placeholder {
  width: 14px;
  height: 14px;
  flex-shrink: 0;
}

.coll-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  user-select: none;
  -webkit-user-select: none;
}

.badge {
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 500;
  line-height: 1.4;
  background: rgba(0, 0, 0, 0.10);
  color: var(--text-secondary);
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  width: 34px;
  text-align: center;
  user-select: none;
  -webkit-user-select: none;
}
.nav-item.active .badge {
  background: color-mix(in srgb, var(--accent) 16%, transparent);
  color: var(--accent);
}

.new-coll-row { padding: 3px 10px 3px 14px; }
.coll-name-input {
  width: 100%;
  font-size: var(--font-size-sm);
  padding: 4px 8px;
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  box-sizing: border-box;
}
.coll-name-input.inline {
  flex: 1;
  width: auto;
  padding: 1px 5px;
  font-size: inherit;
}
.coll-name-input:focus { outline: none; }
</style>
