<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import {
  currentTranslation,
  translationHistory,
  clearTranslationHistory,
  deleteTranslationEntry,
  regenerateTranslation,
} from '../../stores/translationHistory'
import { useAiStore } from '../../stores/ai'
import { useSettingsStore } from '../../stores/settings'

type View = 'current' | 'history'
const view = ref<View>('current')
const sourceExpanded = ref(false)

const ai = useAiStore()
const settingsStore = useSettingsStore()

// When a new translation starts, switch back to current view and reset expand state.
watch(() => currentTranslation.sourceText, () => {
  view.value = 'current'
  sourceExpanded.value = false
})

function fmtTime(iso: string) {
  const d = new Date(iso)
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}

// ── Copy / regenerate actions ──────────────────────────────────────────────
const copied = ref(false)
async function copyResult() {
  if (!currentTranslation.result) return
  await navigator.clipboard.writeText(currentTranslation.result).catch(() => {})
  copied.value = true
  setTimeout(() => { copied.value = false }, 2000)
}

// ── Usage / cost formatting (mirrors the AI chat footer) ────────────────────
const usdToCnyRate = computed(() => {
  const r = Number(settingsStore.settings.usd_to_cny_rate)
  return Number.isFinite(r) && r > 0 ? r : 7.2
})

function fmtCny(cny: number): string {
  if (cny < 0.01) return '<0.01'
  return cny.toFixed(cny < 1 ? 3 : 2)
}

function formatCostCny(costUsd: number | null | undefined) {
  if (typeof costUsd !== 'number' || !Number.isFinite(costUsd) || costUsd < 0) return ''
  const cny = costUsd * usdToCnyRate.value
  return fmtCny(cny)
}

// DeepSeek-style peak hours in Beijing time (UTC+8): 09:00–12:00 & 14:00–18:00.
function isPeakHour(date: Date): boolean {
  const minutes = ((date.getUTCHours() + 8) % 24) * 60 + date.getUTCMinutes()
  const h = minutes / 60
  return (h >= 9 && h < 12) || (h >= 14 && h < 18)
}

// Estimated CNY cost for providers that don't return a cost (e.g. DeepSeek),
// using the configured per-million prices. Returns null when unavailable.
const estimatedCostCny = computed<number | null>(() => {
  const inTok = currentTranslation.inputTokens
  const outTok = currentTranslation.outputTokens
  if (typeof inTok !== 'number' || typeof outTok !== 'number') return null
  const provider = ai.settings.providers.find(p => p.id === currentTranslation.providerId)
  const m = provider?.models.find(x => x.id === currentTranslation.modelId)
  if (!m || (m.input_price_per_million == null && m.output_price_per_million == null)) return null
  const peak = !!m.peak_pricing && isPeakHour(new Date())
  const inPrice = (peak && m.peak_input_price_per_million != null ? m.peak_input_price_per_million : m.input_price_per_million) ?? 0
  const outPrice = (peak && m.peak_output_price_per_million != null ? m.peak_output_price_per_million : m.output_price_per_million) ?? 0
  const cacheHit = currentTranslation.cacheHitTokens ?? 0
  const cacheMiss = Math.max(0, inTok - cacheHit)
  const cacheHitPrice = m.cache_hit_input_price_per_million != null ? m.cache_hit_input_price_per_million : inPrice
  const cost = (cacheMiss / 1e6) * inPrice + (cacheHit / 1e6) * cacheHitPrice + (outTok / 1e6) * outPrice
  return Number.isFinite(cost) && cost > 0 ? cost : null
})

const hasUsage = computed(() =>
  typeof currentTranslation.inputTokens === 'number' || typeof currentTranslation.outputTokens === 'number'
)

const modelLabel = computed(() => currentTranslation.modelName || '')

// Model brand icons (same set/matching as the AI chat).
const modelSvgModules = import.meta.glob<{ default: string }>('/src/assets/models/*.svg', { eager: true })
const modelIconMap: Record<string, string> = {}
for (const [path, mod] of Object.entries(modelSvgModules)) {
  modelIconMap[path.replace(/^.*\//, '').replace(/\.svg$/, '')] = mod.default
}

function modelLogo(modelId = '', providerName = '', providerId = '') {
  const haystack = `${modelId} ${providerName} ${providerId}`.toLowerCase()
  if (haystack.includes('deepseek')) return modelIconMap.deepseek
  if (haystack.includes('claude') || haystack.includes('anthropic')) return modelIconMap.claude
  if (haystack.includes('gemma')) return modelIconMap.gemma
  if (haystack.includes('gemini') || haystack.includes('google')) return modelIconMap.gemini
  if (haystack.includes('qwen') || haystack.includes('通义') || haystack.includes('alibaba')) return modelIconMap.qwen ?? modelIconMap.alibaba
  if (haystack.includes('kimi') || haystack.includes('moonshot')) return modelIconMap.kimi
  if (haystack.includes('grok') || haystack.includes('xai')) return modelIconMap.grok ?? modelIconMap.xai
  if (haystack.includes('zhipu') || haystack.includes('智谱') || haystack.includes('glm')) return modelIconMap.zhipu
  if (haystack.includes('baidu') || haystack.includes('ernie')) return modelIconMap.baidu
  if (haystack.includes('doubao') || haystack.includes('bytedance')) return modelIconMap.bytedance
  if (haystack.includes('mistral') || haystack.includes('huggingface')) return modelIconMap.huggingface
  if (haystack.includes('gpt') || haystack.includes('openai')) return modelIconMap.openai
  if (haystack.includes('ollama')) return modelIconMap['ollama-color']
  for (const key of Object.keys(modelIconMap)) {
    if (haystack.includes(key)) return modelIconMap[key]
  }
  return ''
}

const modelIcon = computed(() =>
  modelLogo(currentTranslation.modelId, currentTranslation.providerName, currentTranslation.providerId)
)
</script>

<template>
  <div class="trans-tab">
    <!-- Header -->
    <div class="list-toolbar">
      <span class="list-heading">{{ view === 'history' ? '翻译记录' : '翻译' }}</span>
      <button
        v-if="view === 'history' && translationHistory.entries.length > 0"
        class="icon-btn"
        title="清空记录"
        @click="clearTranslationHistory"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polyline points="3 6 5 6 21 6"/>
          <path d="M19 6l-1 14H6L5 6"/>
          <path d="M10 11v6M14 11v6"/>
          <path d="M9 6V4h6v2"/>
        </svg>
      </button>
      <button
        v-if="view === 'history'"
        class="icon-btn"
        title="返回当前翻译"
        @click="view = 'current'"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 6 6 18M6 6l12 12"/>
        </svg>
      </button>
      <button
        v-else
        class="icon-btn"
        title="翻译记录"
        @click="view = 'history'"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M12 8v4l3 3"/>
          <circle cx="12" cy="12" r="9"/>
        </svg>
      </button>
    </div>

    <!-- Current translation view -->
    <div v-if="view === 'current'" class="current-view">
      <div v-if="!currentTranslation.sourceText" class="empty">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/>
          <path d="M2 5h12"/><path d="M7 2h1"/>
          <path d="m22 22-5-10-5 10"/><path d="M14 18h6"/>
        </svg>
        <p>暂无翻译</p>
        <span>在 PDF 中选中文字后点击翻译</span>
      </div>

      <template v-else>
        <!-- Source text -->
        <div class="section-label source-label">
          <span>原文</span>
          <button
            type="button"
            class="source-toggle-btn"
            @click="sourceExpanded = !sourceExpanded"
          >
            <svg v-if="!sourceExpanded" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"/></svg>
            <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="18 15 12 9 6 15"/></svg>
            {{ sourceExpanded ? '折叠原文' : '展开原文' }}
          </button>
        </div>
        <div
          class="source-text selectable-text"
          :class="{ 'source-collapsed': !sourceExpanded }"
        >{{ currentTranslation.sourceText }}</div>

        <div class="divider" />

        <!-- Translation result -->
        <div class="section-label">
          译文
          <span v-if="currentTranslation.loading" class="loading-dot" />
        </div>
        <div class="result-text selectable-text">
          <template v-if="currentTranslation.error">
            <span class="error-text">{{ currentTranslation.error }}</span>
          </template>
          <template v-else>
            {{ currentTranslation.result }}
            <span v-if="currentTranslation.loading" class="cursor">▋</span>
          </template>
        </div>

        <!-- Footer: actions (left) + model / token / cost usage (right) -->
        <div v-if="currentTranslation.result || currentTranslation.error" class="msg-footer">
          <div class="msg-actions">
            <button class="action-btn" :class="{ done: copied }" title="复制译文" :disabled="!currentTranslation.result" @click="copyResult">
              <svg v-if="copied" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
              <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>
                <path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>
              </svg>
            </button>
            <button class="action-btn" title="重新生成" :disabled="currentTranslation.loading" @click="regenerateTranslation">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M21 12a9 9 0 1 1-2.64-6.36"/>
                <path d="M21 3v6h-6"/>
              </svg>
            </button>
          </div>
          <div v-if="modelLabel || hasUsage || currentTranslation.error" class="msg-usage">
            <span v-if="modelLabel" class="usage-model" :title="modelLabel">
              <img v-if="modelIcon" :src="modelIcon" class="usage-model-icon" alt="" />
              {{ modelLabel }}
            </span>
            <span v-if="currentTranslation.costUsd != null && formatCostCny(currentTranslation.costUsd)" class="usage-cost" :title="`约 ¥${formatCostCny(currentTranslation.costUsd)} / $${currentTranslation.costUsd.toFixed(6)}`">¥{{ formatCostCny(currentTranslation.costUsd) }}</span>
            <span v-else-if="currentTranslation.costUsd == null && estimatedCostCny != null" class="usage-cost usage-cost-est" :title="`按配置单价估算，约 ¥${estimatedCostCny.toFixed(6)}`">≈¥{{ fmtCny(estimatedCostCny) }}</span>
            <span v-if="currentTranslation.error" class="error-badge">出错</span>
          </div>
        </div>
      </template>
    </div>

    <!-- History view -->
    <template v-else>
      <div v-if="translationHistory.entries.length === 0" class="empty">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M12 8v4l3 3"/><circle cx="12" cy="12" r="9"/>
        </svg>
        <p>暂无翻译记录</p>
      </div>

      <ul v-else class="entry-list">
        <li v-for="entry in translationHistory.entries" :key="entry.id" class="entry-item">
          <div class="entry-header">
            <span class="entry-time">{{ fmtTime(entry.createdAt) }}</span>
            <button class="entry-delete-btn" title="删除" @click="deleteTranslationEntry(entry.id)">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>
          <div class="entry-source selectable-text">{{ entry.sourceText }}</div>
          <div class="entry-divider" />
          <div class="entry-result selectable-text">{{ entry.result }}</div>
          <div v-if="entry.modelName || entry.costUsd != null" class="entry-usage">
            <span v-if="entry.modelName" class="usage-model" :title="entry.modelName">
              <img v-if="modelLogo(entry.modelName, entry.providerName)" :src="modelLogo(entry.modelName, entry.providerName)" class="usage-model-icon" alt="" />
              {{ entry.modelName }}
            </span>
            <span v-if="entry.costUsd != null && formatCostCny(entry.costUsd)" class="usage-cost">¥{{ formatCostCny(entry.costUsd) }}</span>
          </div>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.trans-tab {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.list-toolbar {
  display: flex;
  align-items: center;
  height: 40px;
  padding: 0 10px 0 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  gap: 4px;
}

.list-heading {
  flex: 1;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  background: transparent;
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.icon-btn:hover { background: var(--bg-tertiary); color: var(--text-secondary); }

/* ── Current translation view ── */
.current-view {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  padding: 14px;
  gap: 8px;
}

.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  padding: 24px;
}
.empty p { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-secondary); margin: 0; }
.empty span { font-size: var(--font-size-xs); text-align: center; }

.section-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.source-label {
  justify-content: space-between;
}
.source-toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  font-weight: 600;
  letter-spacing: 0;
  text-transform: none;
  transition: background 0.1s, color 0.1s;
}
.source-toggle-btn:hover {
  background: var(--bg-secondary);
  color: var(--text-secondary);
}

.loading-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
  animation: pulse 1s ease-in-out infinite;
}

.source-text {
  font-size: var(--font-size-md);
  color: var(--text-secondary);
  line-height: 1.6;
  white-space: pre-wrap;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: 10px 12px;
  border: 1px solid var(--border-subtle);
  overflow-y: auto;
}
.source-text.source-collapsed {
  /* ~10 lines: 14px * 1.6 * 10 + 20px padding */
  max-height: 244px;
}

.divider {
  height: 1px;
  background: var(--border-subtle);
  flex-shrink: 0;
}

.result-text {
  font-size: var(--font-size-md);
  color: var(--text-primary);
  line-height: 1.7;
  white-space: pre-wrap;
  min-height: 60px;
}

.cursor {
  display: inline-block;
  animation: blink 0.8s step-end infinite;
  color: var(--accent);
  font-size: 0.9em;
  line-height: 1;
}

.error-text { color: #e53e3e; }

/* ── History list ── */
.entry-list {
  list-style: none;
  overflow-y: auto;
  flex: 1;
  padding: 6px 0;
}

.entry-item {
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-subtle);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.entry-item:last-child { border-bottom: none; }

.entry-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.entry-time {
  font-size: 11px;
  color: var(--text-tertiary);
}

.entry-delete-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  opacity: 0;
  transition: opacity 0.1s, background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.entry-item:hover .entry-delete-btn { opacity: 1; }
.entry-delete-btn:hover { background: var(--bg-hover); color: #e53e3e; }

.entry-source {
  font-size: var(--font-size-md);
  color: var(--text-secondary);
  line-height: 1.5;
  white-space: pre-wrap;
}

.entry-divider {
  height: 1px;
  background: var(--border-subtle);
}

.entry-result {
  font-size: var(--font-size-md);
  color: var(--text-primary);
  line-height: 1.6;
  white-space: pre-wrap;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}

@keyframes blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}

/* ── Footer: actions + model/token/cost usage (mirrors the AI chat footer) ── */
.msg-footer {
  display: flex;
  align-items: center;
  min-height: 22px;
  gap: 4px;
  padding-top: 2px;
  justify-content: space-between;
}

.msg-actions {
  display: flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.action-btn {
  width: 24px;
  height: 24px;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background .1s ease, color .1s ease;
  flex-shrink: 0;
}
.action-btn:hover:not(:disabled) {
  color: var(--text-primary);
  background: var(--bg-hover);
}
.action-btn:disabled { opacity: .4; cursor: not-allowed; }
.action-btn.done { color: #22c55e; }

.msg-usage {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  margin-left: auto;
  padding-left: 8px;
  justify-content: flex-end;
  flex: 1;
  min-width: 0;
  font-size: 10.5px;
  color: var(--text-tertiary);
}
.usage-model {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
  word-break: break-word;
}
.usage-model-icon {
  width: 13px;
  height: 13px;
  border-radius: 3px;
  object-fit: contain;
  flex-shrink: 0;
}
.usage-tokens { color: var(--text-tertiary); }
.usage-cost {
  color: var(--text-secondary);
  font-weight: 500;
  margin-left: 2px;
}
.usage-cost-est { color: var(--text-tertiary); font-weight: 400; }
.error-badge { color: #ef4444; }

.entry-usage {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 5px;
  font-size: 10.5px;
  color: var(--text-tertiary);
  margin-top: 2px;
}
</style>
