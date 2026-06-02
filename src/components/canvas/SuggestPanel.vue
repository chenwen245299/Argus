<script setup lang="ts">
import { ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useAiStore } from '../../stores/ai'
import type { SuggestedEdge } from '../../types'

const props = defineProps<{
  canvasId: string
  paperNames: Record<string, string>  // paper_id → title
}>()

const emit = defineEmits<{
  accept: [suggestion: SuggestedEdge]
  acceptAll: [suggestions: SuggestedEdge[]]
  close: []
}>()

const { t } = useI18n()
const aiStore = useAiStore()

const source = ref<'citations' | 'similarity' | 'ai' | 'all'>('all')
const selectedProviderId = ref('')
const selectedModelId = ref('')
const loading = ref(false)
const error = ref('')
const suggestions = ref<SuggestedEdge[]>([])
const dismissed = ref<Set<string>>(new Set())

const activeSuggestions = computed(() =>
  suggestions.value.filter(s => !dismissed.value.has(suggKey(s)))
)

const providers = computed(() => aiStore.settings?.providers?.filter(p => p.enabled) ?? [])

const selectedProviderModels = computed(() => {
  const p = providers.value.find(p => p.id === selectedProviderId.value)
  return p?.models ?? []
})

function suggKey(s: SuggestedEdge) {
  return `${s.from_paper_id}→${s.to_paper_id}`
}

function shortTitle(paperId: string) {
  const title = props.paperNames[paperId] ?? paperId
  return title.length > 30 ? title.slice(0, 28) + '…' : title
}

function sourceLabel(s: string) {
  const map: Record<string, string> = {
    citations: t('canvas.sourceCitations'),
    similarity: t('canvas.sourceSimilarity'),
    ai: t('canvas.sourceAi'),
  }
  return map[s] ?? s
}

function sourceBadgeClass(s: string) {
  return {
    citations: 'badge-citations',
    similarity: 'badge-similarity',
    ai: 'badge-ai',
  }[s] ?? 'badge-default'
}

async function analyze() {
  if (!props.canvasId) return
  loading.value = true
  error.value = ''
  suggestions.value = []
  dismissed.value = new Set()

  try {
    const result = await invoke<SuggestedEdge[]>('suggest_canvas_edges', {
      canvasId: props.canvasId,
      source: source.value,
      aiProviderId: (source.value === 'ai' || source.value === 'all') && selectedProviderId.value
        ? selectedProviderId.value : null,
      aiModelId: selectedModelId.value || null,
    })
    suggestions.value = result
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

function accept(s: SuggestedEdge) {
  emit('accept', s)
  dismissed.value = new Set([...dismissed.value, suggKey(s)])
}

function reject(s: SuggestedEdge) {
  dismissed.value = new Set([...dismissed.value, suggKey(s)])
}

function acceptAll() {
  emit('acceptAll', activeSuggestions.value)
  activeSuggestions.value.forEach(s => dismissed.value.add(suggKey(s)))
}

function rejectAll() {
  activeSuggestions.value.forEach(s => dismissed.value.add(suggKey(s)))
}
</script>

<template>
  <div class="suggest-panel">
    <div class="suggest-header">
      <span class="suggest-title">{{ t('canvas.suggestEdges') }}</span>
      <button class="close-btn" @click="emit('close')">
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
          <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
        </svg>
      </button>
    </div>

    <div class="suggest-body">
      <!-- Source selector -->
      <div class="field-row">
        <label class="field-label">{{ t('canvas.suggestSource') }}</label>
        <select v-model="source" class="field-select">
          <option value="all">{{ t('canvas.sourceAll') }}</option>
          <option value="citations">{{ t('canvas.sourceCitations') }}</option>
          <option value="similarity">{{ t('canvas.sourceSimilarity') }}</option>
          <option value="ai">{{ t('canvas.sourceAi') }}</option>
        </select>
      </div>

      <!-- AI Provider (for ai/all sources) -->
      <template v-if="source === 'ai' || source === 'all'">
        <div class="field-row">
          <label class="field-label">{{ t('canvas.selectProvider') }}</label>
          <select v-model="selectedProviderId" class="field-select">
            <option value="">{{ t('canvas.noProviders') }}</option>
            <option v-for="p in providers" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
        </div>
        <div v-if="selectedProviderModels.length" class="field-row">
          <label class="field-label">模型</label>
          <select v-model="selectedModelId" class="field-select">
            <option value="">默认</option>
            <option v-for="m in selectedProviderModels" :key="m.id" :value="m.id">{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <button class="analyze-btn" :disabled="loading" @click="analyze">
        <span v-if="loading" class="spinner" />
        {{ loading ? t('canvas.suggesting') : t('canvas.suggestEdges') }}
      </button>

      <!-- Error -->
      <p v-if="error" class="error-msg">{{ error }}</p>

      <!-- Results -->
      <template v-if="suggestions.length > 0">
        <div class="results-header">
          <span class="results-count">{{ t('canvas.suggestions').replace('{count}', String(activeSuggestions.length)) }}</span>
          <div class="results-actions">
            <button class="micro-btn" @click="acceptAll" :disabled="activeSuggestions.length === 0">{{ t('canvas.acceptAll') }}</button>
            <button class="micro-btn micro-btn--danger" @click="rejectAll" :disabled="activeSuggestions.length === 0">{{ t('canvas.rejectAll') }}</button>
          </div>
        </div>

        <div class="suggestions-list">
          <div
            v-for="s in suggestions"
            :key="suggKey(s)"
            class="suggestion-item"
            :class="{ 'suggestion-item--dismissed': dismissed.has(suggKey(s)) }"
          >
            <template v-if="!dismissed.has(suggKey(s))">
              <div class="sugg-flow">
                <span class="sugg-paper">{{ shortTitle(s.from_paper_id) }}</span>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2">
                  <line x1="5" y1="12" x2="19" y2="12"/>
                  <polyline points="12 5 19 12 12 19"/>
                </svg>
                <span class="sugg-paper">{{ shortTitle(s.to_paper_id) }}</span>
              </div>
              <div class="sugg-meta">
                <span class="source-badge" :class="sourceBadgeClass(s.source)">{{ sourceLabel(s.source) }}</span>
                <span v-if="s.score != null" class="sugg-score">{{ (s.score * 100).toFixed(0) }}%</span>
              </div>
              <p v-if="s.reason" class="sugg-reason">{{ s.reason }}</p>
              <div class="sugg-actions">
                <button class="accept-btn" @click="accept(s)">✓ {{ t('canvas.accept') }}</button>
                <button class="reject-btn" @click="reject(s)">✕ {{ t('canvas.reject') }}</button>
              </div>
            </template>
            <div v-else class="dismissed-label">已处理</div>
          </div>
        </div>
      </template>

      <p v-else-if="!loading && suggestions.length === 0 && !error" class="no-results">
        {{ t('canvas.noSuggestions') }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.suggest-panel {
  width: 300px;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  border-left: 1px solid var(--border-subtle);
  height: 100%;
  overflow: hidden;
  font-size: 13px;
}

.suggest-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 14px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.suggest-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}

.close-btn {
  color: var(--text-tertiary);
  border-radius: var(--radius-sm);
  padding: 3px;
  display: flex;
  align-items: center;
}
.close-btn:hover { color: var(--text-primary); background: var(--bg-hover); }

.suggest-body {
  flex: 1;
  overflow-y: auto;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.field-row { display: flex; flex-direction: column; gap: 4px; }
.field-label { font-size: 11px; font-weight: 600; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.04em; }
.field-select {
  width: 100%;
  padding: 5px 8px;
  font-size: 12px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-primary);
}

.analyze-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 7px 14px;
  font-size: 12px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-md);
  transition: opacity 0.12s;
}
.analyze-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.error-msg { font-size: 12px; color: #e53e3e; line-height: 1.4; }

.results-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.results-count { font-size: 11px; font-weight: 600; color: var(--text-tertiary); }
.results-actions { display: flex; gap: 6px; }
.micro-btn {
  padding: 3px 8px;
  font-size: 11px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-pill);
  color: var(--text-secondary);
  background: var(--bg-secondary);
}
.micro-btn:hover:not(:disabled) { background: var(--bg-hover); }
.micro-btn:disabled { opacity: 0.35; cursor: default; }
.micro-btn--danger { color: #e53e3e; border-color: #e53e3e; }
.micro-btn--danger:hover:not(:disabled) { background: #fef2f2; }

.suggestions-list { display: flex; flex-direction: column; gap: 8px; }

.suggestion-item {
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: 10px;
  background: var(--bg-secondary);
  transition: opacity 0.15s;
}
.suggestion-item--dismissed { opacity: 0.4; }
.dismissed-label { font-size: 11px; color: var(--text-tertiary); text-align: center; }

.sugg-flow {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
}
.sugg-paper {
  flex: 1;
  font-size: 11px;
  font-weight: 500;
  color: var(--text-primary);
  line-height: 1.3;
}
.sugg-meta { display: flex; align-items: center; gap: 6px; margin-bottom: 4px; }

.source-badge {
  font-size: 10px;
  font-weight: 500;
  padding: 1px 6px;
  border-radius: var(--radius-pill);
}
.badge-citations { background: #dbeafe; color: #1d4ed8; }
.badge-similarity { background: #d1fae5; color: #065f46; }
.badge-ai { background: #ede9fe; color: #5b21b6; }
.badge-default { background: var(--bg-tertiary); color: var(--text-tertiary); }

.sugg-score { font-size: 10px; color: var(--text-tertiary); }

.sugg-reason {
  font-size: 11px;
  color: var(--text-secondary);
  line-height: 1.4;
  margin: 4px 0;
}

.sugg-actions { display: flex; gap: 6px; margin-top: 6px; }
.accept-btn {
  flex: 1;
  padding: 4px 8px;
  font-size: 11px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-md);
}
.accept-btn:hover { opacity: 0.85; }
.reject-btn {
  flex: 1;
  padding: 4px 8px;
  font-size: 11px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  background: var(--bg-primary);
}
.reject-btn:hover { background: var(--bg-hover); }

.no-results { font-size: 12px; color: var(--text-tertiary); text-align: center; padding: 20px 0; }

.spinner {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid rgba(255,255,255,0.4);
  border-top-color: #fff;
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
</style>
