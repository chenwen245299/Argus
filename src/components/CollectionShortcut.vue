<script setup lang="ts">
import { computed } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { useCollectionsStore } from '../stores/collections'
import { fluentIconFor, fluentReady } from '../utils/fluentEmoji'
import type { Collection } from '../types'

const props = defineProps<{ collection: Collection; active: boolean; dragOver?: boolean }>()
defineEmits<{
  select: [col: Collection]
  togglePin: [col: Collection]
  openCtx: [e: MouseEvent, col: Collection]
}>()
const { t } = useI18n()
const collections = useCollectionsStore()
const ancestors = computed(() => collections.collectionAncestors(props.collection.id))
const parentPath = computed(() => ancestors.value.map(c => c.name).join(' / '))
const fullPath = computed(() => [...ancestors.value.map(c => c.name), props.collection.name].join(' / '))
const emoji = computed(() => (ancestors.value[0] ?? props.collection).emoji?.trim() || '📚')
const emojiIcon = computed(() => fluentReady.value ? fluentIconFor(emoji.value) : null)
</script>

<template>
  <div
    class="pinned-collection-row"
    :class="{ active, 'drag-over': dragOver }"
    :data-collection-id="collection.id"
    @contextmenu.prevent="$emit('openCtx', $event, collection)"
  >
    <button
      class="pinned-collection-open"
      :title="fullPath"
      :aria-current="active ? 'page' : undefined"
      @click="$emit('select', collection)"
    >
      <span class="shortcut-emoji" aria-hidden="true">
        <Icon v-if="emojiIcon" :icon="emojiIcon" width="15" height="15" />
        <template v-else>{{ emoji }}</template>
      </span>
      <span class="shortcut-label">
        <span class="shortcut-name">{{ collection.name }}</span>
        <span v-if="parentPath" class="shortcut-path">{{ parentPath }}</span>
      </span>
      <span class="shortcut-count">{{ collections.collectionPaperCount(collection.id) }}</span>
    </button>
    <button
      class="shortcut-unpin"
      :title="t('collections.unpin')"
      :aria-label="t('collections.unpinNamed', { name: collection.name })"
      :disabled="collections.isPinPending(collection.id)"
      @click.stop="$emit('togglePin', collection)"
    >
      <Icon icon="fluent:pin-16-filled" width="13" height="13" />
    </button>
  </div>
</template>

<style scoped>
.pinned-collection-row {
  display: flex;
  align-items: center;
  margin: 2px 6px;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  transition: background 0.12s, color 0.12s;
}
.pinned-collection-row:hover { background: var(--bg-hover); color: var(--text-primary); }
.pinned-collection-row.active { background: var(--bg-active); color: var(--accent); }
.pinned-collection-row.drag-over { outline: 1.5px solid var(--accent); outline-offset: -2px; background: var(--bg-active); color: var(--accent); }
.pinned-collection-open {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
  padding: 6px 0 6px 9px;
  text-align: left;
  color: inherit;
}
.shortcut-emoji { display: flex; align-items: center; justify-content: center; flex: 0 0 15px; font-size: 13px; }
.shortcut-label { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 1px; }
.shortcut-name, .shortcut-path { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.shortcut-name { font-size: 13px; line-height: 18px; }
.active .shortcut-name { font-weight: 500; }
.shortcut-path { font-size: 10px; line-height: 14px; color: var(--text-tertiary); }
.shortcut-count { flex-shrink: 0; min-width: 20px; padding: 1px 6px; border-radius: var(--radius-pill); font-size: 10px; text-align: center; background: var(--bg-hover); color: var(--text-secondary); }
.active .shortcut-count { background: color-mix(in srgb, var(--accent) 12%, transparent); color: var(--accent); }
.shortcut-unpin {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 24px;
  height: 24px;
  margin: 0 3px;
  color: var(--accent);
  opacity: 0.65;
  border-radius: var(--radius-sm);
}
.shortcut-unpin:hover { opacity: 1; background: var(--bg-active); }
.shortcut-unpin:disabled { opacity: 0.4; cursor: wait; }
button:focus-visible { outline: 2px solid var(--accent); outline-offset: -1px; border-radius: var(--radius-sm); }
</style>
