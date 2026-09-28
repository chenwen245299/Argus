<script setup lang="ts">
// The "服务商繁忙 · N 秒后自动重试" badge of a paused arXiv batch. Its own
// component so the 1 s countdown re-renders only this badge: in ArxivView the
// tick re-ran the whole view, the unvirtualised paper list included.
import { ref, computed, watch, onUnmounted } from 'vue'
import { Icon } from '@iconify/vue'
import type { AnalysisWaiting } from '../stores/arxiv'

const props = defineProps<{ waiting: AnalysisWaiting }>()

const now = ref(Date.now())
let ticker: ReturnType<typeof setInterval> | null = null

function stop() {
  if (ticker) { clearInterval(ticker); ticker = null }
}

watch(() => props.waiting, () => {
  now.value = Date.now()
  stop()
  ticker = setInterval(() => {
    now.value = Date.now()
    // Nothing left to count once the pause is over.
    if (now.value >= props.waiting.until) stop()
  }, 1000)
}, { immediate: true })

onUnmounted(stop)

function formatWait(seconds: number): string {
  if (seconds < 60) return `${seconds} 秒`
  const m = Math.floor(seconds / 60)
  const s = seconds % 60
  return s > 0 ? `${m} 分 ${s} 秒` : `${m} 分钟`
}

const reason = computed(() =>
  props.waiting.message ? `服务商繁忙：${props.waiting.message}` : '服务商繁忙')

const countdown = computed(() => {
  const left = Math.max(0, Math.ceil((props.waiting.until - now.value) / 1000))
  const c = props.waiting.concurrency
  const conc = c > 0 ? `（并发 ${c}）` : ''
  return left > 0 ? `· ${formatWait(left)}后自动重试${conc}` : `· 正在重试${conc}`
})
</script>

<template>
  <span class="analysis-waiting" :title="`${reason} ${countdown}`" data-tauri-drag-region>
    <Icon icon="fluent:clock-24-regular" width="13" height="13" class="analysis-waiting-icon" data-tauri-drag-region />
    <span class="analysis-waiting-reason" data-tauri-drag-region>{{ reason }}</span>
    <span class="analysis-waiting-count" data-tauri-drag-region>{{ countdown }}</span>
  </span>
</template>

<style scoped>
.analysis-waiting {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
}
.analysis-waiting-icon { flex-shrink: 0; color: #f59e0b; }
/* Only the throttling reason gives way when the bar runs out of room. */
.analysis-waiting-reason {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.analysis-waiting-count {
  flex-shrink: 0;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
</style>
