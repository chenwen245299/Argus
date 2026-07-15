<script setup lang="ts">
import { computed, ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLibraryStore } from '../stores/library'
import type { PaperIndexEntry } from '../types'

const props = defineProps<{
  /** Slug of the paper we're linking FROM (excluded from results). */
  currentSlug: string
  /** Paper ids to hide from results (self + already-related). */
  excludeIds: string[]
}>()
const emit = defineEmits<{
  (e: 'select', entry: PaperIndexEntry): void
  (e: 'cancel'): void
}>()

const { t } = useI18n()
const library = useLibraryStore()

const query = ref('')
const inputEl = ref<HTMLInputElement | null>(null)
onMounted(() => inputEl.value?.focus())

const exclude = computed(() => new Set(props.excludeIds))
const results = computed(() => {
  const q = query.value.trim().toLowerCase()
  return library.papers
    .filter(p => p.slug !== props.currentSlug && !exclude.value.has(p.id))
    .filter(p =>
      !q ||
      p.title.toLowerCase().includes(q) ||
      (p.authors ?? []).some(a => a.toLowerCase().includes(q)))
    .slice(0, 50)
})

function authorLine(e: PaperIndexEntry) {
  const first = e.authors?.[0] ?? ''
  const etal = (e.authors?.length ?? 0) > 1 ? ' et al.' : ''
  const yr = e.year ? ` · ${e.year}` : ''
  return `${first}${etal}${yr}`
}
</script>

<template>
  <div class="paper-picker">
    <input
      ref="inputEl"
      v-model="query"
      class="pp-input"
      type="text"
      :placeholder="t('related.searchPlaceholder')"
      @keydown.escape.stop.prevent="emit('cancel')"
    />
    <div v-if="results.length === 0" class="pp-empty">{{ t('related.noMatches') }}</div>
    <div v-else class="pp-list">
      <button
        v-for="e in results"
        :key="e.id"
        class="pp-item"
        @click="emit('select', e)"
      >
        <span class="pp-item-title">{{ e.title }}</span>
        <span class="pp-item-sub">{{ authorLine(e) }}</span>
      </button>
    </div>
    <button class="pp-back" @click="emit('cancel')">{{ t('related.back') }}</button>
  </div>
</template>

<style scoped>
.paper-picker {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.pp-input {
  margin: 0 0 8px;
  padding: 7px 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  outline: none;
}
.pp-input:focus {
  border-color: var(--accent);
}
.pp-empty {
  padding: 20px 8px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
.pp-list {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
.pp-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 7px 8px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  text-align: left;
  cursor: pointer;
}
.pp-item:hover {
  background: var(--bg-hover);
}
.pp-item-title {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.pp-item-sub {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.pp-back {
  margin-top: 8px;
  padding: 6px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  cursor: pointer;
}
.pp-back:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
</style>
