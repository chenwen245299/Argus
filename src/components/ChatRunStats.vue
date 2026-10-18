<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { compactTokenCount, summarizeChatRuns, type ChatRun } from '../utils/chatRunStats'

const props = defineProps<{ turns: number; runs: ChatRun[] }>()
const { t } = useI18n()
const stats = computed(() => summarizeChatRuns(props.runs))
const activity = computed(() => t('chatStats.turns', { n: props.turns }) + (stats.value.calls
  ? ` ${stats.value.callsEstimated ? '≥' : ''}${t('chatStats.steps', { n: stats.value.calls })}` : ''))
</script>

<template>
  <div class="chat-run-stats" :aria-label="t('chatStats.title')">
    <div class="run-stats-content">
      <span v-if="turns || stats.calls || stats.speed" class="run-runtime">
        <span v-if="turns || stats.calls" class="run-stat" :title="t('chatStats.activityHint', { tools: stats.tools })">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 11a6 6 0 1 1 11 0M8 8l3-3M4 11h8" /></svg>
          {{ activity }}
        </span>
        <span v-if="stats.speed" class="run-stat run-speed" :title="t('chatStats.speedHint')">~{{ stats.speed }} tok/s</span>
      </span>
      <span v-if="stats.measured" class="run-usage">
        <span class="run-stat" :title="t('chatStats.tokensHint', { input: stats.input.toLocaleString(), output: stats.output.toLocaleString() })">
          <svg viewBox="0 0 16 16" aria-hidden="true"><ellipse cx="8" cy="3.5" rx="5" ry="2" /><path d="M3 3.5v8c0 2.7 10 2.7 10 0v-8M3 7.5c0 2.7 10 2.7 10 0" /></svg>
          {{ compactTokenCount(stats.total) }} tok
        </span>
        <span v-if="stats.cachePercent !== null" class="run-stat run-cache" :title="t('chatStats.cacheHint')">{{ t('chatStats.cache', { n: stats.cachePercent }) }}</span>
      </span>
    </div>
  </div>
</template>

<style scoped>
.chat-run-stats { container-type: inline-size; display: flex; flex: 1; align-items: center; justify-content: flex-end; min-width: 0; margin-left: 8px; overflow: hidden; color: var(--text-tertiary); font-size: 11px; line-height: 20px; font-variant-numeric: tabular-nums; }
.run-stats-content { display: flex; align-items: center; gap: 10px; white-space: nowrap; }
.run-runtime, .run-usage { display: inline-flex; align-items: center; gap: 8px; }
.run-stat { display: inline-flex; align-items: center; gap: 4px; white-space: nowrap; }
.run-stat svg { width: 13px; height: 13px; fill: none; stroke: currentColor; stroke-width: 1.2; stroke-linecap: round; stroke-linejoin: round; flex-shrink: 0; }
@container (max-width: 320px) { .run-speed { display: none; } }
@container (max-width: 240px) { .run-cache { display: none; } }
@container (max-width: 150px) { .run-runtime { display: none; } }
@container (max-width: 65px) { .run-usage { display: none; } }
</style>
