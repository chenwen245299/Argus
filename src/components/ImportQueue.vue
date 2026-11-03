<script setup lang="ts">
import { computed, ref, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { useImportStore } from '../stores/import'
import type { ImportStatus } from '../types'

const { t } = useI18n()
const imports = useImportStore()
const menu = ref<HTMLDetailsElement>()
const panelStyle = ref<Record<string, string>>({})
function positionPanel() {
  if (!menu.value?.open) return
  const rect = menu.value.querySelector('summary')!.getBoundingClientRect()
  const width = Math.min(420, window.innerWidth - 32)
  const top = rect.bottom + 12
  panelStyle.value = {
    left: `${Math.max(16, Math.min(rect.right - width, window.innerWidth - width - 16))}px`,
    top: `${top}px`, width: `${width}px`,
    maxHeight: `${Math.max(80, window.innerHeight - top - 16)}px`,
    '--queue-list-height': `${Math.max(80, window.innerHeight - top - 80)}px`,
  }
}
onMounted(() => window.addEventListener('resize', positionPanel))
onBeforeUnmount(() => window.removeEventListener('resize', positionPanel))
const finished = computed(() => imports.jobs.length - imports.activeCount)
const failed = computed(() => imports.jobs.filter(j => j.status === 'error').length)
const labels: Record<ImportStatus, string> = {
  queued: 'queued', importing: 'importing', downloading: 'downloading',
  extracting: 'extracting', ai_meta: 'aiMeta', renaming: 'renaming',
  fetching_meta: 'fetchingMeta', fetching_refs: 'fetchingRefs', ranking: 'ranking',
  awaiting_duplicate: 'awaitingDuplicate', skipping: 'skipping', skipped: 'skipped', done: 'done', error: 'error',
}
</script>

<template>
  <details v-if="imports.jobs.length" ref="menu" class="import-queue" @toggle="positionPanel">
    <summary :class="{ 'has-errors': failed > 0 }" :title="t('import.jobsTitle')">
      <span v-if="imports.activeCount" class="queue-spinner" />
      <span>{{ t('import.queueSummary', { finished, total: imports.jobs.length }) }}</span>
      <span v-if="failed"> · {{ t('import.error') }} {{ failed }}</span>
    </summary>
    <div class="queue-panel" :style="panelStyle">
      <div class="queue-header">
        <strong>{{ t('import.jobsTitle') }}</strong>
        <button v-if="finished" type="button" @click="imports.clearDone()">{{ t('import.queueClear') }}</button>
      </div>
      <ul>
        <li v-for="job in imports.jobs" :key="job.id" :data-status="job.status">
          <span class="queue-filename" :title="job.filename">{{ job.filename }}</span>
          <span class="queue-status">{{ t(`import.${labels[job.status]}`) }}</span>
          <p v-if="job.error" class="queue-error">{{ job.error }}</p>
          <p v-if="job.warning" class="queue-warning">{{ job.warning }}</p>
        </li>
      </ul>
    </div>
  </details>
</template>

<style scoped>
.import-queue { position: relative; flex-shrink: 0; font-size: var(--font-size-xs); }
summary { display: flex; align-items: center; gap: 5px; color: var(--text-secondary); cursor: pointer; list-style: none; }
summary::-webkit-details-marker { display: none; }
.has-errors, .queue-error, li[data-status="error"] .queue-status { color: var(--color-danger, #c33); }
.queue-spinner { width: 10px; height: 10px; border: 1.5px solid var(--border-subtle); border-top-color: var(--accent); border-radius: 50%; animation: queue-spin .75s linear infinite; }
.queue-panel { position: fixed; box-sizing: border-box; display: flex; flex-direction: column; z-index: 1000; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); box-shadow: var(--shadow-lg); padding: 12px; overflow: hidden; }
.queue-header { display: flex; flex-shrink: 0; justify-content: space-between; align-items: center; gap: 12px; margin-bottom: 8px; }
.queue-header button { border: 0; background: transparent; color: var(--accent); font: inherit; cursor: pointer; }
ul { list-style: none; margin: 0; padding: 0; min-height: 0; max-height: min(360px, var(--queue-list-height, 65vh)); overflow: auto; }
li { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 4px 12px; padding: 8px 0; border-top: 1px solid var(--border-subtle); }
.queue-filename { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-primary); }
.queue-status { color: var(--text-secondary); }
.queue-error { grid-column: 1 / -1; overflow-wrap: anywhere; user-select: text; }
.queue-warning { grid-column: 1 / -1; white-space: pre-line; overflow-wrap: anywhere; color: var(--text-secondary); user-select: text; }
@keyframes queue-spin { to { transform: rotate(360deg); } }
</style>
