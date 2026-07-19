<script setup lang="ts">
import { Icon } from '@iconify/vue'
import { useCollectionsStore } from '../stores/collections'
import type { Collection } from '../types'

defineOptions({ name: 'CollectionCascadeMenu' })

const props = withDefaults(defineProps<{
  collections: Collection[]
  ancestors?: string[]
  level?: number
}>(), {
  ancestors: () => [],
  level: 0,
})

const emit = defineEmits<{
  select: [id: string]
}>()

const collectionsStore = useCollectionsStore()

function collectionEmoji(col: Collection): string {
  return col.emoji?.trim() || '📚'
}

function childCollections(col: Collection): Collection[] {
  if (props.ancestors.includes(col.id)) return []
  return collectionsStore.childrenOf(col.id)
}
</script>

<template>
  <div class="ctx-submenu" :class="{ 'is-root': level === 0, 'is-nested': level > 0 }">
    <div v-for="col in collections" :key="col.id" class="ctx-folder-row-wrap">
      <button
        class="ctx-item ctx-collection-item"
        :class="{ 'ctx-toplevel-folder': level === 0 }"
        :style="level === 0 ? 'cursor: default' : ''"
        @click.stop="level > 0 && emit('select', col.id)"
      >
        <span class="ctx-collection-icon" aria-hidden="true">{{ collectionEmoji(col) }}</span>
        <span class="ctx-collection-name">{{ col.name }}</span>
        <Icon
          v-if="childCollections(col).length"
          class="ctx-folder-chevron"
          icon="fluent:chevron-right-24-regular"
          width="11"
          height="11"
        />
      </button>

      <CollectionCascadeMenu
        v-if="childCollections(col).length"
        :collections="childCollections(col)"
        :ancestors="[...ancestors, col.id]"
        :level="level + 1"
        @select="emit('select', $event)"
      />
    </div>
  </div>
</template>

<style scoped>
:global(.ctx-toplevel-folder) {
  opacity: 0.6;
  pointer-events: auto;
}
:global(.ctx-toplevel-folder:hover) {
  background: transparent !important;
  color: var(--text-primary) !important;
}
</style>
