<script setup lang="ts">
// 「最近过滤」: papers the analysis moved out of the inbox for scoring below the
// filter threshold (inbox/filtered.json, newest first). They used to be deleted
// outright, so a strict model looked exactly like a broken one — rows vanished
// and nothing said why. Here they can be read, with the model's reason, and put
// back.
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { useArxivStore } from '../stores/arxiv'
import type { ArxivFilteredPaper } from '../types'

const emit = defineEmits<{ (e: 'close'): void }>()
const store = useArxivStore()

const loading = ref(true)
const error = ref('')
const restoring = ref(new Set<string>())
const expanded = ref<string | null>(null)

async function reload() {
  await store.loadFiltered()
  loading.value = false
}

onMounted(reload)

// A batch that is still dropping papers writes them every couple of seconds;
// follow along while the panel is open.
let reloadTimer: ReturnType<typeof setTimeout> | null = null
watch(() => store.analyzeCounts.filtered, (n, old) => {
  if (n <= old || reloadTimer) return
  reloadTimer = setTimeout(() => {
    reloadTimer = null
    reload()
  }, 2500)
})
onUnmounted(() => { if (reloadTimer) clearTimeout(reloadTimer) })

const papers = computed(() => store.filteredPapers)

async function restore(p: ArxivFilteredPaper) {
  if (restoring.value.has(p.arxiv_id)) return
  error.value = ''
  restoring.value = new Set(restoring.value).add(p.arxiv_id)
  try {
    await store.restoreFiltered([p.arxiv_id])
  } catch (e) {
    error.value = String(e)
  } finally {
    const next = new Set(restoring.value)
    next.delete(p.arxiv_id)
    restoring.value = next
  }
}

async function clearAll() {
  if (!window.confirm('清空「最近过滤」记录？这些论文不会回到收件箱。')) return
  error.value = ''
  try {
    await store.clearFiltered()
  } catch (e) {
    error.value = String(e)
  }
}

function openUrl(url: string) {
  if (url) invoke('open_url', { url }).catch(console.error)
}

function formatScore(score: number | null): string {
  if (score === null) return '—'
  const v = Math.min(10, Math.max(0, score))
  return Number.isInteger(v) ? String(v) : v.toFixed(1)
}

function formatWhen(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getMonth() + 1}月${d.getDate()}日 ${pad(d.getHours())}:${pad(d.getMinutes())}`
}
</script>

<template>
  <div class="filtered-panel">
    <div class="fp-header">
      <div class="fp-heading">
        <span class="fp-title">最近过滤</span>
        <span class="fp-sub">相关度低于阈值、已移出收件箱的论文，保留最近 500 篇。恢复后保留原评分，不会再被阈值移出。</span>
      </div>
      <button class="fp-icon-btn" title="关闭" aria-label="关闭" @click="emit('close')">
        <Icon icon="fluent:dismiss-24-regular" width="15" height="15" />
      </button>
    </div>

    <div v-if="papers.length > 0 || error" class="fp-toolbar">
      <span class="fp-count">共 {{ papers.length }} 篇</span>
      <span v-if="error" class="fp-error">{{ error }}</span>
      <span class="fp-spacer" />
      <button v-if="papers.length > 0" class="fp-text-btn" @click="clearAll">清空记录</button>
    </div>

    <div class="fp-body">
      <div v-if="loading" class="fp-empty"><span class="fp-spinner" /></div>
      <div v-else-if="papers.length === 0" class="fp-empty">
        <Icon icon="fluent:filter-dismiss-24-regular" width="24" height="24" class="fp-empty-icon" />
        <p>还没有被过滤的论文。</p>
        <p class="fp-empty-hint">开启「低于阈值自动过滤」后，AI 分析时相关度低于阈值的论文会移出收件箱，并记录在这里。</p>
      </div>
      <template v-else>
      <div
        v-for="p in papers"
        :key="p.arxiv_id"
        class="fp-row"
        :class="{ open: expanded === p.arxiv_id }"
      >
        <div class="fp-score">{{ formatScore(p.relevance_score) }}</div>
        <div class="fp-main" @click="expanded = expanded === p.arxiv_id ? null : p.arxiv_id">
          <div class="fp-paper-title">{{ p.title }}</div>
          <div v-if="p.relevance_reason" class="fp-reason">{{ p.relevance_reason }}</div>
          <div class="fp-meta">
            <span v-for="topic in (p.matched_topics ?? [])" :key="topic" class="fp-topic">{{ topic }}</span>
            <span>低于 {{ formatScore(p.filter_threshold) }} 分</span>
            <span v-if="formatWhen(p.filtered_at)">· {{ formatWhen(p.filtered_at) }}</span>
          </div>
          <div v-if="expanded === p.arxiv_id" class="fp-details" @click.stop>
            <p v-if="p.analysis_summary" class="fp-summary">{{ p.analysis_summary }}</p>
            <p class="fp-abstract">{{ p.summary }}</p>
            <button class="fp-text-btn" @click="openUrl(p.abs_url)">
              <Icon icon="fluent:open-24-regular" width="12" height="12" />
              打开原文页
            </button>
          </div>
        </div>
        <button
          class="fp-restore-btn"
          :disabled="restoring.has(p.arxiv_id)"
          title="放回收件箱"
          @click="restore(p)"
        >
          <Icon icon="fluent:arrow-undo-24-regular" width="13" height="13" />
          恢复
        </button>
      </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.filtered-panel {
  display: flex;
  flex-direction: column;
  min-height: 0;
  height: 100%;
}
.fp-header {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 14px 16px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.fp-heading { display: flex; flex-direction: column; gap: 4px; flex: 1; min-width: 0; }
.fp-title { font-size: 14px; font-weight: 600; color: var(--text-primary); }
.fp-sub { font-size: 12px; line-height: 1.5; color: var(--text-secondary); }
.fp-icon-btn {
  width: 26px; height: 26px;
  display: inline-flex; align-items: center; justify-content: center;
  flex-shrink: 0;
  border-radius: var(--radius-md);
  color: var(--text-tertiary);
}
.fp-icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.fp-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 16px;
  border-bottom: 1px solid var(--border-subtle);
  font-size: 12px;
  color: var(--text-secondary);
  flex-shrink: 0;
}
.fp-count { flex-shrink: 0; }
.fp-error {
  min-width: 0;
  color: #ef4444;
  overflow-wrap: anywhere;
  user-select: text;
  -webkit-user-select: text;
}
.fp-spacer { flex: 1; }
.fp-text-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  padding: 3px 8px;
  border-radius: var(--radius-md);
  font-size: 12px;
  color: var(--text-secondary);
}
.fp-text-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.fp-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 6px 8px 10px;
}
.fp-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  padding: 36px 24px;
  text-align: center;
  font-size: 13px;
  color: var(--text-secondary);
}
.fp-empty p { margin: 0; }
.fp-empty-icon { color: var(--text-tertiary); }
.fp-empty-hint { font-size: 12px; line-height: 1.5; color: var(--text-tertiary); max-width: 360px; }
.fp-spinner {
  width: 14px; height: 14px;
  border: 1.5px solid var(--border-default);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: fp-spin 0.8s linear infinite;
}
@keyframes fp-spin { to { transform: rotate(360deg); } }

.fp-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 9px 8px;
  border-radius: var(--radius-md);
}
.fp-row:hover,
.fp-row.open { background: var(--bg-hover); }
.fp-score {
  flex-shrink: 0;
  min-width: 26px;
  padding: 2px 0;
  border-radius: var(--radius-sm);
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  font-size: 12px;
  font-weight: 600;
  text-align: center;
  font-variant-numeric: tabular-nums;
}
.fp-main { flex: 1; min-width: 0; cursor: pointer; }
.fp-paper-title {
  font-size: 13px;
  font-weight: 500;
  line-height: 1.45;
  color: var(--text-primary);
}
.fp-reason {
  margin-top: 3px;
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-secondary);
}
.fp-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 6px;
  margin-top: 4px;
  font-size: 11px;
  color: var(--text-tertiary);
}
.fp-topic {
  padding: 0 6px;
  border-radius: var(--radius-pill);
  background: var(--bg-tertiary);
  color: var(--text-secondary);
}
.fp-details {
  margin-top: 8px;
  cursor: auto;
  user-select: text;
  -webkit-user-select: text;
}
.fp-details .fp-text-btn { margin-left: -8px; }
.fp-summary,
.fp-abstract {
  margin: 0 0 8px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-secondary);
}
.fp-abstract { color: var(--text-tertiary); }
.fp-restore-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  height: 26px;
  padding: 0 9px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  font-size: 12px;
  color: var(--text-secondary);
}
.fp-restore-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.fp-restore-btn:disabled { opacity: 0.5; cursor: default; }
</style>
