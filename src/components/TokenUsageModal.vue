<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import type { AiSettingsInfo, AppSettings } from '../types'

const { t } = useI18n()

interface UsageRecord {
  ts: string
  source: string
  provider: string
  model: string
  input_tokens: number
  output_tokens: number
  cost_usd?: number
  /** Portion of input_tokens served from the provider's prompt cache. */
  cache_hit_tokens?: number
}

type Range = 'today' | 'week' | 'month' | 'year'

const emit = defineEmits<{ close: [] }>()

const records = ref<UsageRecord[]>([])
const aiSettings = ref<AiSettingsInfo | null>(null)
const appSettings = ref<AppSettings | null>(null)
const loading = ref(false)
const range = ref<Range>('week')

const RANGES: { key: Range }[] = [
  { key: 'today' },
  { key: 'week' },
  { key: 'month' },
  { key: 'year' },
]

const BAR_STACK_HEIGHT = 96
const DEFAULT_USD_TO_CNY_RATE = 7.2

// ── Price lookup ───────────────────────────────────────────────────────────────

const priceMap = computed(() => {
  const map = new Map<string, { inputCny?: number; outputCny?: number; inputUsd?: number; outputUsd?: number; peakPricing?: boolean; peakInputCny?: number; peakOutputCny?: number }>()
  for (const p of (aiSettings.value?.providers ?? [])) {
    for (const m of p.models) {
      map.set(`${p.id}::${m.id}`, {
        inputCny: m.input_price_per_million,
        outputCny: m.output_price_per_million,
        inputUsd: m.input_price_usd_per_million,
        outputUsd: m.output_price_usd_per_million,
        peakPricing: m.peak_pricing,
        peakInputCny: m.peak_input_price_per_million,
        peakOutputCny: m.peak_output_price_per_million,
      })
    }
  }
  return map
})

// DeepSeek-style peak hours in Beijing time (UTC+8): 09:00–12:00 & 14:00–18:00.
// Everything else is off-peak. Uses UTC so it's correct regardless of the user's
// local timezone.
function isPeakHour(date: Date): boolean {
  const minutes = ((date.getUTCHours() + 8) % 24) * 60 + date.getUTCMinutes()
  const h = minutes / 60
  return (h >= 9 && h < 12) || (h >= 14 && h < 18)
}

const usdToCnyRate = computed(() => {
  const rate = Number(appSettings.value?.usd_to_cny_rate)
  return Number.isFinite(rate) && rate > 0 ? rate : DEFAULT_USD_TO_CNY_RATE
})

// Map provider ID -> display name
const providerNameMap = computed(() => {
  const map = new Map<string, string>()
  for (const p of (aiSettings.value?.providers ?? [])) {
    map.set(p.id, p.name)
  }
  return map
})

function providerName(id: string) {
  return providerNameMap.value.get(id) ?? id
}

// ── Model icon matching ───────────────────────────────────────────────────────

const modelIconModules = import.meta.glob('../assets/models/*.svg', {
  eager: true,
  query: '?url',
  import: 'default',
}) as Record<string, string>

const MODEL_ICON_RULES: [string[], string][] = [
  [['deepseek'], 'deepseek.svg'],
  [['gpt', 'openai', 'o1', 'o3', 'o4'], 'openai.svg'],
  [['claude', 'anthropic'], 'claude.svg'],
  [['gemini'], 'gemini.svg'],
  [['gemma'], 'gemma.svg'],
  [['qwen', 'tongyi', 'dashscope', 'alibaba'], 'qwen.svg'],
  [['kimi', 'moonshot'], 'kimi.svg'],
  [['grok', 'xai'], 'grok.svg'],
  [['glm', 'chatglm', 'zhipu'], 'zhipu.svg'],
  [['z-ai', 'zai'], 'z-ai.svg'],
  [['ernie', 'wenxin', 'baidu', 'qianfan'], 'baidu.svg'],
  [['doubao', 'volc', 'bytedance'], 'bytedance.svg'],
  [['hunyuan', 'tencent'], 'tencent.svg'],
  [['azure', 'microsoft'], 'microsoft.svg'],
  [['llama', 'ollama'], 'ollama-color.svg'],
  [['huggingface', 'hf-'], 'huggingface.svg'],
  [['nvidia'], 'nvidia.svg'],
  [['kling'], 'kling.svg'],
  [['mi-', 'xiaomi'], 'xiaomi.svg'],
]

function modelIconUrl(model: string, provider: string): string | null {
  const haystack = `${model} ${provider} ${providerName(provider)}`.toLowerCase()
  const matched = MODEL_ICON_RULES.find(([keywords]) => keywords.some(k => haystack.includes(k)))
  if (!matched) return null
  return modelIconModules[`../assets/models/${matched[1]}`] ?? null
}

function recordCost(r: UsageRecord) {
  if (r.cost_usd != null && Number.isFinite(r.cost_usd) && r.cost_usd >= 0) {
    return r.cost_usd * usdToCnyRate.value
  }
  const key = `${r.provider}::${r.model}`
  const prices = priceMap.value.get(key)
  if (!prices) return 0
  let c = 0
  if (prices.inputUsd != null || prices.outputUsd != null) {
    if (prices.inputUsd != null)  c += (r.input_tokens  / 1_000_000) * prices.inputUsd * usdToCnyRate.value
    if (prices.outputUsd != null) c += (r.output_tokens / 1_000_000) * prices.outputUsd * usdToCnyRate.value
    return c
  }
  // Peak/off-peak: price each record by whether its own timestamp falls in the
  // peak window (falls back to the standard price if the peak price isn't set).
  const peak = prices.peakPricing && isPeakHour(new Date(r.ts))
  const inputCny = peak && prices.peakInputCny != null ? prices.peakInputCny : prices.inputCny
  const outputCny = peak && prices.peakOutputCny != null ? prices.peakOutputCny : prices.outputCny
  if (inputCny != null)  c += (r.input_tokens  / 1_000_000) * inputCny
  if (outputCny != null) c += (r.output_tokens / 1_000_000) * outputCny
  return c
}

function recordHasCostData(r: UsageRecord) {
  if (r.cost_usd != null && Number.isFinite(r.cost_usd) && r.cost_usd >= 0) return true
  const prices = priceMap.value.get(`${r.provider}::${r.model}`)
  return !!prices && (
    prices.inputUsd != null || prices.outputUsd != null ||
    prices.inputCny != null || prices.outputCny != null
  )
}

const hasCostData = computed(() => filteredRecords.value.some(recordHasCostData))

const MODEL_COLORS = [
  '#7EA6F7',
  '#B9A3F6',
  '#F5A6C8',
  '#F2C66D',
  '#7ECDEB',
  '#F3A5A5',
  '#AAB7D6',
  '#C7A8E8',
]

// ── Time range filtering ───────────────────────────────────────────────────────

function rangeStart(r: Range): Date {
  const now = new Date()
  if (r === 'today') {
    return new Date(now.getFullYear(), now.getMonth(), now.getDate())
  }
  if (r === 'week') {
    const d = new Date(now)
    d.setDate(d.getDate() - 6)
    return new Date(d.getFullYear(), d.getMonth(), d.getDate())
  }
  if (r === 'month') {
    return new Date(now.getFullYear(), now.getMonth(), 1)
  }
  // year
  return new Date(now.getFullYear(), 0, 1)
}

const filteredRecords = computed(() => {
  const start = rangeStart(range.value).getTime()
  return records.value.filter(r => new Date(r.ts).getTime() >= start)
})

// ── Summary totals ─────────────────────────────────────────────────────────────

const totalInput  = computed(() => filteredRecords.value.reduce((s, r) => s + r.input_tokens,  0))
const totalOutput = computed(() => filteredRecords.value.reduce((s, r) => s + r.output_tokens, 0))
const totalCost   = computed(() => {
  if (!hasCostData.value) return null
  return filteredRecords.value.reduce((s, r) => s + recordCost(r), 0)
})

// ── Bar chart grouping ─────────────────────────────────────────────────────────

interface BarSegment {
  key: string
  model: string
  provider: string
  total: number
}

interface Bar {
  label: string
  input: number
  output: number
  cost: number
  segments: BarSegment[]
}

function emptyBar(label: string): Bar {
  return { label, input: 0, output: 0, cost: 0, segments: [] }
}

function addRecordToBar(bar: Bar, rec: UsageRecord) {
  bar.input += rec.input_tokens
  bar.output += rec.output_tokens
  bar.cost += recordCost(rec)
  const key = `${rec.provider}::${rec.model}`
  let seg = bar.segments.find(s => s.key === key)
  if (!seg) {
    seg = { key, model: rec.model, provider: rec.provider, total: 0 }
    bar.segments.push(seg)
  }
  seg.total += rec.input_tokens + rec.output_tokens
}

function finalizeBars<T extends Bar>(bars: T[]): T[] {
  for (const bar of bars) {
    bar.segments.sort((a, b) => b.total - a.total)
  }
  return bars
}

const chartBars = computed<Bar[]>(() => {
  const r = range.value
  const now = new Date()

  if (r === 'today') {
    // 24 hours: 0..23
    const buckets = Array.from({ length: 24 }, (_, h): Bar => emptyBar(`${h}`))
    for (const rec of filteredRecords.value) {
      const h = new Date(rec.ts).getHours()
      addRecordToBar(buckets[h], rec)
    }
    return finalizeBars(buckets)

  } else if (r === 'week') {
    // Last 7 days (day-6 .. today)
    const buckets: Bar[] = []
    for (let i = 6; i >= 0; i--) {
      const d = new Date(now)
      d.setDate(d.getDate() - i)
      const key = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`
      buckets.push({ ...emptyBar(String(d.getDate())), _key: key } as Bar & { _key: string })
    }
    for (const rec of filteredRecords.value) {
      const d = new Date(rec.ts)
      const key = `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`
      const b = (buckets as (Bar & { _key: string })[]).find(b => b._key === key)
      if (b) addRecordToBar(b, rec)
    }
    return finalizeBars(buckets)

  } else if (r === 'month') {
    // Each day of current month
    const daysInMonth = new Date(now.getFullYear(), now.getMonth()+1, 0).getDate()
    const buckets: (Bar & { _key: string })[] = Array.from({ length: daysInMonth }, (_, i) => {
      const day = i + 1
      const key = `${now.getFullYear()}-${String(now.getMonth()+1).padStart(2,'0')}-${String(day).padStart(2,'0')}`
      return { ...emptyBar(String(day)), _key: key }
    })
    for (const rec of filteredRecords.value) {
      const d = new Date(rec.ts)
      if (d.getMonth() !== now.getMonth() || d.getFullYear() !== now.getFullYear()) continue
      const b = buckets[d.getDate()-1]
      if (b) addRecordToBar(b, rec)
    }
    return finalizeBars(buckets)

  } else {
    // Year: 12 months
    const MONTHS = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec']
    const buckets: (Bar & { _month: number })[] = Array.from({ length: 12 }, (_, i) => ({
      ...emptyBar(MONTHS[i]), _month: i,
    }))
    for (const rec of filteredRecords.value) {
      const d = new Date(rec.ts)
      if (d.getFullYear() !== now.getFullYear()) continue
      const b = buckets[d.getMonth()]
      if (b) addRecordToBar(b, rec)
    }
    return finalizeBars(buckets)
  }
})

const chartMax = computed(() => {
  const m = Math.max(...chartBars.value.map(b => b.input + b.output), 1)
  return m
})

// ── Time-of-day density (status-page style) ─────────────────────────────────────
// Fold every record onto a single 24h clock, split into 15-min buckets. Each bar's
// color intensity scales with the tokens used in that slot — busier slot, deeper blue.
// This card spans the full modal width, so it needs more (thinner) bars than the
// activity-log strip to get the same slim status-page look.
const HOUR_BUCKET_COUNT = 96
const HOUR_MINUTES_PER_BUCKET = 1440 / HOUR_BUCKET_COUNT

const hourlyDensity = computed(() => {
  const bucketTokens = new Array<number>(HOUR_BUCKET_COUNT).fill(0)
  for (const r of filteredRecords.value) {
    const d = new Date(r.ts)
    const minute = d.getHours() * 60 + d.getMinutes()
    const bi = Math.min(HOUR_BUCKET_COUNT - 1, Math.floor(minute / HOUR_MINUTES_PER_BUCKET))
    bucketTokens[bi] += r.input_tokens + r.output_tokens
  }
  const max = Math.max(...bucketTokens, 1)
  return bucketTokens.map((tokens, i) => {
    if (tokens <= 0) return { tokens: 0, color: '', title: '' }
    const pct = Math.round(30 + (tokens / max) * 70) // 30%..100% blend toward accent
    const startMin = i * HOUR_MINUTES_PER_BUCKET
    const endMin = (i + 1) * HOUR_MINUTES_PER_BUCKET
    const hhmm = (m: number) =>
      `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(Math.round(m % 60)).padStart(2, '0')}`
    return {
      tokens,
      color: `color-mix(in srgb, #5b8def ${pct}%, #e9eef7)`,
      title: `${hhmm(startMin)}–${hhmm(endMin)} · ${fmtT(tokens)} tokens`,
    }
  })
})

const hasHourlyDensity = computed(() => hourlyDensity.value.some(b => b.tokens > 0))

// ── Model ranking ──────────────────────────────────────────────────────────────

interface ModelRow {
  key: string
  model: string
  provider: string
  input: number
  output: number
  total: number
  cost: number
  cacheHit: number
}

const modelRanking = computed<ModelRow[]>(() => {
  const map = new Map<string, ModelRow>()
  for (const r of filteredRecords.value) {
    const key = `${r.provider}::${r.model}`
    if (!map.has(key)) map.set(key, { key, model: r.model, provider: r.provider, input: 0, output: 0, total: 0, cost: 0, cacheHit: 0 })
    const row = map.get(key)!
    row.input  += r.input_tokens
    row.output += r.output_tokens
    row.total  += r.input_tokens + r.output_tokens
    row.cost   += recordCost(r)
    row.cacheHit += r.cache_hit_tokens ?? 0
  }
  return [...map.values()].sort((a, b) => b.total - a.total)
})

// Cache hits are a subset of input tokens, so the rate is over input only.
// Returns null when the model has no cached input (hide the chip entirely).
function modelCacheRate(row: ModelRow): number | null {
  if (row.cacheHit <= 0 || row.input <= 0) return null
  return Math.round((row.cacheHit / row.input) * 100)
}

const rankingMax = computed(() => Math.max(...modelRanking.value.map(r => r.total), 1))

const modelColorMap = computed(() => {
  const map = new Map<string, string>()
  modelRanking.value.forEach((row, i) => {
    map.set(row.key, MODEL_COLORS[i % MODEL_COLORS.length])
  })
  return map
})

function colorForModel(key: string) {
  return modelColorMap.value.get(key) ?? MODEL_COLORS[0]
}

const modelLegend = computed(() =>
  modelRanking.value.slice(0, 6).map(row => ({
    key: row.key,
    model: row.model,
    color: colorForModel(row.key),
  }))
)

// ── Formatting ─────────────────────────────────────────────────────────────────

function fmtT(n: number) {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M'
  if (n >= 1_000)     return (n / 1_000).toFixed(1) + 'K'
  return n === 0 ? '0' : String(n)
}

function fmtCost(n: number) {
  if (n < 0.001) return n === 0 ? '' : '<¥0.01'
  return '¥' + n.toFixed(n < 1 ? 3 : 2)
}

// ── Data loading ───────────────────────────────────────────────────────────────

async function load() {
  loading.value = true
  try {
    const [recs, settings, app] = await Promise.all([
      invoke<UsageRecord[]>('get_token_usage'),
      invoke<AiSettingsInfo>('get_ai_settings').catch(() => null),
      invoke<AppSettings>('get_settings').catch(() => null),
    ])
    records.value = recs
    aiSettings.value = settings
    appSettings.value = app
  } catch { records.value = [] }
  finally { loading.value = false }
}


onMounted(load)
</script>

<template>
  <div class="usage-backdrop" @click.self="emit('close')">
    <div class="usage-modal">

      <!-- Header -->
      <div class="usage-header">
        <span class="usage-title">{{ t('tokenUsage.title') }}</span>
        <div class="header-right">
          <!-- Range selector -->
          <div class="range-tabs">
            <button
              v-for="r in RANGES" :key="r.key"
              class="range-tab" :class="{ active: range === r.key }"
              @click="range = r.key"
            >{{ t('tokenUsage.' + r.key) }}</button>
          </div>
          <button class="btn-close" @click="emit('close')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>
      </div>

      <!-- Loading -->
      <div v-if="loading" class="usage-loading">{{ t('tokenUsage.loading') }}</div>

      <div v-else class="usage-body">

        <!-- Summary cards -->
        <div class="summary-cards">
          <div class="summary-card">
            <span class="card-label">{{ t('tokenUsage.totalInput') }}</span>
            <span class="card-value input-color">{{ fmtT(totalInput) }}</span>
          </div>
          <div class="summary-card">
            <span class="card-label">{{ t('tokenUsage.totalOutput') }}</span>
            <span class="card-value output-color">{{ fmtT(totalOutput) }}</span>
          </div>
          <div v-if="totalCost !== null" class="summary-card">
            <span class="card-label">{{ t('tokenUsage.cost') }}</span>
            <span class="card-value cost-color">{{ totalCost < 0.001 ? '¥0.00' : '¥' + totalCost.toFixed(2) }}</span>
          </div>
          <div class="summary-card dim">
            <span class="card-label">{{ t('tokenUsage.calls') }}</span>
            <span class="card-value">{{ filteredRecords.length }}</span>
          </div>
        </div>

        <!-- Bar chart -->
        <div class="chart-card time-chart-card">
          <div class="chart-head">
            <div>
              <div class="chart-title">{{ t('tokenUsage.byTime') }}</div>
              <div class="chart-subtitle">{{ t('tokenUsage.byTimeSub') }}</div>
            </div>
          </div>

          <div v-if="filteredRecords.length === 0" class="chart-empty">{{ t('tokenUsage.noData') }}</div>
          <div v-else class="chart-wrap">
            <div class="bars-area">
              <div
                v-for="(bar, i) in chartBars"
                :key="i"
                class="bar-col"
              >
                <!-- Value label on top -->
                <span v-if="bar.input + bar.output > 0" class="bar-label">{{ fmtT(bar.input + bar.output) }}</span>

                <!-- Stacked bar: output on top, input below -->
                <div
                  class="bar-stack"
                  :class="{ 'only-input': bar.input > 0 && bar.output === 0, 'only-output': bar.output > 0 && bar.input === 0 }"
                  :style="{ height: BAR_STACK_HEIGHT + 'px' }"
                >
                  <div
                    v-for="segment in bar.segments"
                    :key="segment.key"
                    class="bar-seg model-seg"
                    :style="{
                      height: chartMax > 0 ? ((segment.total / chartMax) * BAR_STACK_HEIGHT) + 'px' : '0',
                      background: colorForModel(segment.key),
                    }"
                  />
                </div>

                <!-- Cost below -->
                <span v-if="hasCostData && bar.cost > 0" class="bar-cost">{{ fmtCost(bar.cost) }}</span>

                <span class="bar-x">{{ bar.label }}</span>
              </div>
            </div>

            <!-- Legend -->
            <div class="chart-legend">
              <template v-for="item in modelLegend" :key="item.key">
                <span class="legend-dot" :style="{ background: item.color }" />{{ item.model }}
              </template>
              <span v-if="modelRanking.length > modelLegend.length" class="legend-hint">+{{ modelRanking.length - modelLegend.length }}</span>
              <template v-if="!hasCostData">
                <span class="legend-hint">{{ t('tokenUsage.priceHint') }}</span>
              </template>
            </div>
          </div>
        </div>

        <!-- Time-of-day density (status-page style) -->
        <div v-if="hasHourlyDensity" class="chart-card hour-density-card">
          <div class="chart-head">
            <div>
              <div class="chart-title">{{ t('tokenUsage.byHour') }}</div>
              <div class="chart-subtitle">{{ t('tokenUsage.byHourSub') }}</div>
            </div>
          </div>
          <div class="hour-bars">
            <span
              v-for="(b, i) in hourlyDensity"
              :key="i"
              class="hour-bar"
              :class="{ empty: !b.tokens }"
              :title="b.title"
              :style="b.tokens ? { background: b.color } : {}"
            />
          </div>
          <div class="hour-labels">
            <span>0</span><span>6</span><span>12</span><span>18</span><span>24</span>
          </div>
        </div>

        <!-- Model ranking -->
        <div v-if="modelRanking.length > 0" class="chart-card model-chart-card">
          <div class="chart-head">
            <div>
              <div class="chart-title">{{ t('tokenUsage.byModel') }}</div>
              <div class="chart-subtitle">{{ t('tokenUsage.byModelSub') }}</div>
            </div>
          </div>
          <div class="model-list">
            <div v-for="row in modelRanking" :key="row.key" class="model-ranking-row">
              <div class="model-info">
                <span class="model-icon-wrap">
                  <img v-if="modelIconUrl(row.model, row.provider)" :src="modelIconUrl(row.model, row.provider)!" class="model-icon" alt="" />
                  <span v-else class="model-icon-fallback">{{ row.model.slice(0, 1).toUpperCase() }}</span>
                </span>
                <div class="model-text">
                  <span class="model-name">{{ row.model }}</span>
                  <span class="model-provider">{{ providerName(row.provider) }}</span>
                </div>
              </div>
              <div class="model-mid">
                <div class="model-bar-wrap">
                  <div
                    class="model-bar"
                    :style="{
                      width: (row.total / rankingMax * 100) + '%',
                      background: colorForModel(row.key),
                    }"
                  />
                </div>
                <span
                  v-if="modelCacheRate(row) !== null"
                  class="model-cache"
                  :title="t('tokenUsage.cacheHitRate')"
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="13 2 4 14 12 14 11 22 20 10 12 10 13 2" />
                  </svg>
                  {{ modelCacheRate(row) }}%
                </span>
              </div>
              <div class="model-nums">
                <span class="model-total">{{ fmtT(row.total) }}</span>
                <span v-if="row.cost > 0" class="model-cost cost-color">{{ fmtCost(row.cost) }}</span>
              </div>
            </div>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
.usage-backdrop {
  position: fixed; inset: 0;
  background: rgba(30, 41, 59, 0.32);
  backdrop-filter: blur(10px) saturate(1.05);
  -webkit-backdrop-filter: blur(10px) saturate(1.05);
  display: flex; align-items: center; justify-content: center;
  z-index: 9000;
}
.usage-modal {
  --usage-bg: #fbfdff;
  --usage-panel: #ffffff;
  --usage-soft: #f4f8ff;
  --usage-border: #e1e8f2;
  --usage-border-strong: #d3deeb;
  --usage-text: #1f2937;
  --usage-muted: #64748b;
  --usage-faint: #94a3b8;
  --usage-blue: #5b8def;
  --usage-blue-soft: #eaf2ff;
  --usage-purple: #8f7cf6;
  --usage-purple-soft: #f1eeff;
  --usage-orange: #f5a524;
  --usage-orange-soft: #fff4df;
  background: var(--usage-bg);
  color: var(--usage-text);
  border: 1px solid rgba(226, 232, 240, 0.92);
  border-radius: 16px;
  width: 780px; max-width: 96vw;
  max-height: 86vh;
  display: flex; flex-direction: column;
  box-shadow: 0 28px 72px rgba(15, 23, 42, 0.18);
  overflow: hidden;
}

/* Header */
.usage-header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 18px 16px 22px;
  border-bottom: 1px solid var(--usage-border);
  flex-shrink: 0;
  gap: 10px;
  background: #ffffff;
}
.usage-title { font-size: 16px; font-weight: 650; flex-shrink: 0; color: var(--usage-text); }
.header-right { display: flex; align-items: center; gap: 8px; }

.range-tabs {
  display: flex;
  background: #f0f4fa;
  border: 1px solid #e4ebf5;
  border-radius: 999px;
  padding: 3px;
  gap: 1px;
}
.range-tab {
  font-size: 12px; font-weight: 550;
  padding: 4px 11px;
  border-radius: 999px;
  border: none; background: transparent;
  color: var(--usage-muted);
  cursor: pointer;
  transition: all 0.15s;
}
.range-tab:hover { color: var(--usage-text); }
.range-tab.active {
  background: #fff;
  color: #2563eb;
  font-weight: 600;
  box-shadow: 0 1px 5px rgba(30, 64, 175, 0.12);
}

.btn-close {
  width: 26px; height: 26px; flex-shrink: 0;
  display: flex; align-items: center; justify-content: center;
  border-radius: 6px; background: transparent; border: none;
  color: var(--usage-faint); cursor: pointer;
}
.btn-close:hover { background: #eef3fa; color: var(--usage-text); }

/* Loading */
.usage-loading {
  display: flex; align-items: center; justify-content: center;
  padding: 60px; color: var(--usage-muted); font-size: 13px;
}

/* Body */
.usage-body {
  overflow-y: auto; flex: 1;
  padding: 18px 22px 24px;
  display: flex; flex-direction: column; gap: 16px;
  background: #f7faff;
}

/* Summary cards */
.summary-cards {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 12px;
}
.summary-card {
  background: var(--usage-panel);
  border: 1px solid var(--usage-border);
  border-radius: 12px;
  padding: 14px 16px;
  display: flex; flex-direction: column; gap: 5px;
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.04);
}
.summary-card:nth-child(1) { background: #f4f8ff; }
.summary-card:nth-child(2) { background: #f6f3ff; }
.summary-card:nth-child(3) { background: #fff8eb; }
.card-label { font-size: 12px; color: var(--usage-muted); }
.card-value { font-size: 25px; font-weight: 750; color: var(--usage-text); line-height: 1.1; letter-spacing: 0; }
.summary-card.dim .card-value { font-size: 21px; color: var(--usage-muted); }

/* Colors */
.input-color  { color: #3f7ee8; }
.output-color { color: #7c65e8; }
.cost-color   { color: #e58b00; }

/* Chart card */
.chart-card {
  background: rgba(255, 255, 255, 0.84);
  border: 1px solid var(--usage-border);
  border-radius: 14px;
  padding: 16px 18px 14px;
  display: flex; flex-direction: column;
  flex-shrink: 0;
  box-shadow: 0 10px 28px rgba(15, 23, 42, 0.045);
}
.time-chart-card {
  height: 260px;
}
.model-chart-card {
  min-height: 150px;
  height: auto;
}
.chart-head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
  margin-bottom: 12px;
}
.chart-title {
  font-size: 13px; font-weight: 650;
  color: var(--usage-text);
  flex-shrink: 0;
}
.chart-subtitle {
  margin-top: 2px;
  font-size: 11px;
  color: var(--usage-faint);
}
.chart-empty {
  flex: 1;
  display: flex; align-items: center; justify-content: center;
  font-size: 12px; color: var(--usage-faint);
}

/* Bars */
.chart-wrap { display: flex; flex-direction: column; gap: 8px; flex: 1; overflow: visible; }
.bars-area {
  flex: 1;
  display: flex;
  align-items: flex-end;
  gap: 7px;
  overflow-x: auto;
  overflow-y: hidden;
  padding: 18px 2px 6px;
  border-radius: 10px;
  background: #ffffff;
}
.bars-area::-webkit-scrollbar { height: 3px; }
.bars-area::-webkit-scrollbar-thumb { background: var(--usage-border-strong); border-radius: 2px; }
.bar-col {
  display: flex; flex-direction: column; align-items: center;
  flex: 1; min-width: 38px; max-width: 54px;
  height: 100%;
  justify-content: flex-end;
  gap: 5px;
}
.bar-label {
  min-height: 14px;
  font-size: 10px; font-weight: 600; color: var(--usage-muted);
  white-space: nowrap; line-height: 1; flex-shrink: 0;
  padding: 2px 6px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.76);
  box-shadow: 0 1px 4px rgba(15, 23, 42, 0.06);
}
.bar-stack {
  width: 62%; max-width: 34px; min-width: 12px;
  display: flex; flex-direction: column;
  justify-content: flex-end; gap: 0; flex-shrink: 0;
  border-radius: 6px;
  overflow: hidden;
}
.bar-seg { width: 100%; min-height: 0; transition: height 0.3s ease; }
.model-seg:first-child { border-radius: 6px 6px 0 0; }
.model-seg:last-child { border-radius: 0 0 6px 6px; }
.model-seg:only-child { border-radius: 6px; }
.bar-input  { background: #7EA6F7; border-radius: 0 0 5px 5px; }
.bar-output { background: #B9A3F6; border-radius: 5px 5px 0 0; }
.bar-stack.only-input .bar-input { border-radius: 5px; }
.bar-stack.only-output .bar-output { border-radius: 5px; }
.bar-cost {
  font-size: 9px; color: #e58b00; font-weight: 550; white-space: nowrap; flex-shrink: 0;
}
.bar-x { font-size: 10px; color: var(--usage-faint); line-height: 1; flex-shrink: 0; }

/* Legend */
.chart-legend {
  display: flex; align-items: center; gap: 8px;
  flex-wrap: wrap;
  font-size: 12px; color: var(--usage-muted);
}
.legend-dot {
  display: inline-block;
  width: 8px; height: 8px; border-radius: 2px;
  flex-shrink: 0;
}
.input-bg  { background: var(--usage-blue); }
.output-bg { background: var(--usage-purple); }
.legend-hint { margin-left: 6px; font-size: 10px; font-style: italic; }

/* Time-of-day density card */
.hour-density-card { min-height: 0; height: auto; }
.hour-bars {
  display: flex; align-items: stretch; gap: 2px;
  height: 30px;
}
.hour-bar {
  flex: 1 1 0;
  min-width: 0;
  border-radius: 3px;
  background: #e9eef7;
  transition: transform 0.1s ease, filter 0.1s ease;
}
.hour-bar:not(.empty) { box-shadow: 0 1px 2px rgba(15, 23, 42, 0.1); }
.hour-bar:not(.empty):hover { transform: scaleY(1.12); filter: saturate(1.12); }
.hour-labels {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  margin-top: 5px;
  font-size: 10px;
  color: var(--usage-faint);
}
.hour-labels span:nth-child(2),
.hour-labels span:nth-child(3),
.hour-labels span:nth-child(4) { text-align: center; }
.hour-labels span:last-child { text-align: right; }

/* Model ranking */
.model-list { display: flex; flex-direction: column; gap: 12px; }
.model-ranking-row {
  display: grid;
  grid-template-columns: minmax(210px, 1.2fr) minmax(180px, 1fr) 86px;
  align-items: center;
  gap: 14px;
  padding: 4px 0;
}
.model-info {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.model-icon-wrap {
  width: 32px;
  height: 32px;
  border-radius: 9px;
  background: #eef4ff;
  border: 1px solid rgba(211, 222, 235, 0.9);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.model-icon {
  width: 21px;
  height: 21px;
  object-fit: contain;
  display: block;
}
.model-icon-fallback {
  font-size: 13px;
  font-weight: 700;
  color: var(--usage-blue);
}
.model-text {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.model-name {
  font-size: 13px; font-weight: 600; color: var(--usage-text);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.model-provider { font-size: 11px; color: var(--usage-faint); }
.model-mid {
  display: flex; align-items: center; gap: 10px; min-width: 0;
}
.model-bar-wrap {
  flex: 1 1 auto; min-width: 0;
  height: 8px; background: #e9f0fb;
  border-radius: 999px; overflow: hidden;
}
.model-cache {
  display: inline-flex; align-items: center; gap: 3px;
  flex-shrink: 0;
  font-size: 11px; font-weight: 600;
  color: var(--usage-text-soft, #94a3b8);
}
.model-cache svg { color: #5b8def; }
.model-bar {
  height: 100%; background: var(--usage-blue); border-radius: 999px;
  transition: width 0.4s ease;
  min-width: 2px;
}
.model-nums {
  display: flex; flex-direction: column; align-items: flex-end; gap: 1px;
}
.model-total { font-size: 13px; font-weight: 650; color: var(--usage-text); }
.model-cost  { font-size: 11px; }
</style>
