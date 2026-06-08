<script setup lang="ts">
import { ref, computed, onMounted, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { DEFAULT_ARXIV_ANALYSIS_PROMPT, useArxivStore } from '../../stores/arxiv'
import { useAiStore } from '../../stores/ai'
import type { ArxivConfig } from '../../types'

const props = defineProps<{ standalone?: boolean }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()
const store = useArxivStore()
const aiStore = useAiStore()

const tlHover = ref(false)

const form = ref<ArxivConfig>(normalizeConfig(store.config))
const saving = ref(false)
const saveMsg = ref('')
const newCategory = ref('')

let formReady = false
let skipAutoSave = false
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null

function normalizeConfig(config: ArxivConfig): ArxivConfig {
  return {
    ...config,
    ai_analysis_prompt: config.ai_analysis_prompt?.trim()
      ? config.ai_analysis_prompt
      : DEFAULT_ARXIV_ANALYSIS_PROMPT,
    ai_filter_enabled: config.ai_filter_enabled ?? true,
    ai_filter_threshold: Number.isFinite(config.ai_filter_threshold)
      ? Math.min(10, Math.max(0, config.ai_filter_threshold))
      : 6,
    ai_analysis_concurrency: Number.isInteger(config.ai_analysis_concurrency)
      ? Math.min(10, Math.max(1, config.ai_analysis_concurrency))
      : 5,
    fetch_biorxiv: config.fetch_biorxiv ?? false,
    fetch_arxiv: config.fetch_arxiv ?? true,
  }
}

// Providers with configured keys can be used for arXiv analysis.
const availableProviders = computed(() =>
  (aiStore.settings.providers ?? []).filter((p: any) => p.enabled && p.has_key)
)

const selectedProvider = computed(() =>
  availableProviders.value.find((p: any) => p.id === form.value.ai_provider_id)
)

const availableModels = computed(() => selectedProvider.value?.models ?? [])

const keywordsDraft = ref('')

function syncKeywordsDraftFromForm() {
  keywordsDraft.value = form.value.keywords.join('\n')
}

function flushKeywordsDraft() {
  form.value.keywords = keywordsDraft.value.split('\n').filter(Boolean)
}

onMounted(async () => {
  await aiStore.load()
  if (!store.loaded) await store.load()
  form.value = normalizeConfig(store.config)
  syncKeywordsDraftFromForm()
  await nextTick()
  formReady = true
})

watch(form, () => {
  if (!formReady || skipAutoSave) return
  if (autoSaveTimer) clearTimeout(autoSaveTimer)
  autoSaveTimer = setTimeout(() => save(), 600)
}, { deep: true })

function addCategory() {
  const cat = newCategory.value.trim()
  if (cat && !form.value.categories.includes(cat)) {
    form.value.categories.push(cat)
  }
  newCategory.value = ''
}

function removeCategory(cat: string) {
  form.value.categories = form.value.categories.filter(c => c !== cat)
}

async function save() {
  if (saving.value) return
  saving.value = true
  saveMsg.value = ''
  try {
    const next = normalizeConfig(form.value)
    await store.saveConfig(next)
    await store.setAutoFetch(next.auto_fetch_enabled)
    skipAutoSave = true
    form.value = next
    await nextTick()
    skipAutoSave = false
    saveMsg.value = t('arxivSettings.saved')
    setTimeout(() => saveMsg.value = '', 2000)
  } catch (e) {
    saveMsg.value = String(e)
  } finally {
    saving.value = false
  }
}

function resetPrompt() {
  form.value.ai_analysis_prompt = DEFAULT_ARXIV_ANALYSIS_PROMPT
}

// Preset categories
const PRESETS = ['cs.AI', 'cs.CL', 'cs.LG', 'cs.CV', 'cs.NE', 'cs.RO', 'stat.ML', 'cs.IR', 'cs.MA', 'eess.AS']
</script>

<template>
  <div class="arxiv-settings">
    <!-- Modal header with traffic lights (only when shown as modal from ArxivView) -->
    <div class="settings-header" v-if="props.standalone">
      <div class="traffic-lights" @mouseenter="tlHover = true" @mouseleave="tlHover = false">
        <button class="tl-btn tl-red" @click="emit('close')">
          <svg v-if="tlHover" width="6" height="6" viewBox="0 0 10 10" fill="none" stroke="#4d0000" stroke-width="1.8" stroke-linecap="round">
            <line x1="2" y1="2" x2="8" y2="8"/><line x1="8" y1="2" x2="2" y2="8"/>
          </svg>
        </button>
        <button class="tl-btn tl-yellow">
          <svg v-if="tlHover" width="6" height="6" viewBox="0 0 10 2" fill="none" stroke="#5c3d00" stroke-width="1.8" stroke-linecap="round">
            <line x1="1" y1="1" x2="9" y2="1"/>
          </svg>
        </button>
        <button class="tl-btn tl-green">
          <svg v-if="tlHover" width="7" height="7" viewBox="0 0 10 10" fill="none" stroke="#003300" stroke-width="1.5" stroke-linecap="round">
            <path d="M2 5 L4.5 7.5 L8.5 2.5"/>
          </svg>
        </button>
      </div>
      <span class="settings-title">{{ t('arxivSettings.title') }}</span>
      <div class="tl-spacer" />
    </div>

    <div class="settings-body">
    <h2 v-if="!props.standalone" class="section-title">{{ t('arxivSettings.title') }}</h2>
    <p class="section-desc">{{ t('arxivSettings.desc') }}</p>

    <!-- Auto-fetch toggle (prominent) -->
    <div class="field-row highlight-row">
      <div>
        <label class="field-label">{{ t('arxivSettings.autoFetch') }}</label>
        <p class="field-hint">{{ t('arxivSettings.autoFetchHint') }}</p>
      </div>
      <label class="toggle">
        <input type="checkbox" v-model="form.auto_fetch_enabled" />
        <span class="toggle-track" />
      </label>
    </div>

    <!-- Source toggles: arXiv and bioRxiv -->
    <div class="source-toggles">
      <div class="source-toggle-row arxiv-toggle-row">
        <div>
          <label class="field-label source-label">
            <span class="source-pill arxiv-pill-color">arXiv</span>
            爬取 arXiv
          </label>
          <p class="field-hint">从 arXiv 按分类抓取论文预印本</p>
        </div>
        <label class="toggle">
          <input type="checkbox" v-model="form.fetch_arxiv" />
          <span class="toggle-track" />
        </label>
      </div>

      <div class="source-toggle-row biorxiv-toggle-row">
        <div>
          <label class="field-label source-label">
            <span class="source-pill biorxiv-pill-color">bioRxiv</span>
            爬取 bioRxiv
          </label>
          <p class="field-hint">按日期全量抓取生命科学预印本，与 arXiv 合并显示</p>
        </div>
        <label class="toggle">
          <input type="checkbox" v-model="form.fetch_biorxiv" />
          <span class="toggle-track" />
        </label>
      </div>
    </div>

    <!-- Categories (only relevant when arXiv is enabled) -->
    <div class="field-group" :class="{ 'section-dimmed': !form.fetch_arxiv }">
      <label class="field-label">{{ t('arxivSettings.categories') }}</label>
      <div class="cat-tags">
        <span v-for="cat in form.categories" :key="cat" class="cat-tag">
          {{ cat }}
          <button @click="removeCategory(cat)">×</button>
        </span>
      </div>
      <div class="cat-presets">
        <span class="hint-label">{{ t('arxivSettings.presets') }}:</span>
        <button
          v-for="p in PRESETS"
          :key="p"
          class="preset-btn"
          :class="{ active: form.categories.includes(p) }"
          @click="form.categories.includes(p) ? removeCategory(p) : form.categories.push(p)"
        >{{ p }}</button>
      </div>
      <div class="add-cat-row">
        <input
          v-model="newCategory"
          class="field-input"
          :placeholder="t('arxivSettings.categoryPh')"
          @keydown.enter="addCategory"
        />
        <button class="btn-ghost" @click="addCategory">{{ t('arxivSettings.add') }}</button>
      </div>
    </div>

    <!-- Keywords -->
    <div class="field-group">
      <label class="field-label">{{ t('arxivSettings.keywords') }}</label>
      <textarea
        v-model="keywordsDraft"
        class="field-textarea"
        rows="3"
        :placeholder="t('arxivSettings.keywordsPh')"
        @blur="flushKeywordsDraft"
      />
      <p class="field-hint">{{ t('arxivSettings.keywordsHint') }}</p>
    </div>

    <!-- Fetch frequency -->
    <div class="field-row">
      <label class="field-label">{{ t('arxivSettings.frequency') }}</label>
      <select v-model.number="form.interval_days" class="field-input sm">
        <option :value="1">{{ t('arxivSettings.daily') }}</option>
        <option :value="2">{{ t('arxivSettings.every2days') }}</option>
        <option :value="7">{{ t('arxivSettings.weekly') }}</option>
      </select>
    </div>

    <!-- Fetch time -->
    <div class="field-row">
      <label class="field-label">{{ t('arxivSettings.fetchTime') }}</label>
      <input type="time" v-model="form.fetch_time" class="field-input sm" />
    </div>

    <!-- Days back -->
    <div class="field-row">
      <label class="field-label">{{ t('arxivSettings.daysBack') }}</label>
      <input type="number" v-model.number="form.days_back" min="1" max="30" class="field-input sm" />
    </div>


    <!-- AI analysis -->
    <div class="section-sep">{{ t('arxivSettings.aiSection') }}</div>

    <div class="field-row">
      <div>
        <label class="field-label">{{ t('arxivSettings.aiAnalysis') }}</label>
        <p class="field-hint">{{ t('arxivSettings.aiAnalysisHint') }}</p>
      </div>
      <label class="toggle">
        <input type="checkbox" v-model="form.ai_analysis_enabled" />
        <span class="toggle-track" />
      </label>
    </div>

    <div class="field-group">
      <div class="label-row">
        <label class="field-label">{{ t('arxivSettings.analysisPrompt') }}</label>
        <button class="link-btn" @click="resetPrompt">{{ t('arxivSettings.resetPrompt') }}</button>
      </div>
      <textarea
        v-model="form.ai_analysis_prompt"
        class="field-textarea prompt-textarea"
        spellcheck="false"
      />
      <p class="field-hint">{{ t('arxivSettings.analysisPromptHint') }}</p>
    </div>

    <div class="field-row">
      <div>
        <label class="field-label">{{ t('arxivSettings.autoFilter') }}</label>
        <p class="field-hint">{{ t('arxivSettings.autoFilterHint') }}</p>
      </div>
      <label class="toggle">
        <input type="checkbox" v-model="form.ai_filter_enabled" />
        <span class="toggle-track" />
      </label>
    </div>

    <div v-if="form.ai_filter_enabled" class="field-row">
      <label class="field-label">{{ t('arxivSettings.filterThreshold') }}</label>
      <div class="threshold-control">
        <input
          type="number"
          v-model.number="form.ai_filter_threshold"
          min="0"
          max="10"
          step="1"
          class="field-input xs"
        />
        <span class="field-suffix">{{ t('arxivSettings.points') }}</span>
      </div>
    </div>

    <template v-if="form.ai_analysis_enabled">
      <div class="field-group">
        <label class="field-label">{{ t('arxivSettings.aiProvider') }}</label>
        <select v-model="form.ai_provider_id" class="field-input">
          <option :value="null">{{ t('arxivSettings.selectProvider') }}</option>
          <option v-for="p in availableProviders" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
        <p v-if="availableProviders.length === 0" class="field-hint warn">
          {{ t('arxivSettings.noProviders') }}
        </p>
      </div>

      <div class="field-group" v-if="form.ai_provider_id">
        <label class="field-label">{{ t('arxivSettings.aiModel') }}</label>
        <select v-if="availableModels.length > 0" v-model="form.ai_model_id" class="field-input">
          <option :value="null">{{ t('arxivSettings.selectModel') }}</option>
          <option v-for="m in availableModels" :key="m.id" :value="m.id">{{ m.display_name || m.id }}</option>
        </select>
        <input
          v-else
          v-model="form.ai_model_id"
          class="field-input"
          :placeholder="t('arxivSettings.modelPh')"
        />
      </div>

      <!-- Concurrency -->
      <div class="field-group">
        <label class="field-label">同时请求数</label>
        <div class="concurrency-row">
          <input
            type="range"
            v-model.number="form.ai_analysis_concurrency"
            min="1" max="10" step="1"
            class="concurrency-slider"
          />
          <span class="concurrency-value">{{ form.ai_analysis_concurrency }}</span>
        </div>
        <p class="field-hint">同时向 AI 发送的并发请求数量（默认 5），数值越大分析速度越快，但可能触发 API 限流</p>
      </div>
    </template>

    <!-- Save -->
    <div class="action-row">
      <button class="btn-primary" @click="save" :disabled="saving">
        {{ saving ? t('arxivSettings.saving') : t('arxivSettings.save') }}
      </button>
      <span v-if="saveMsg" class="save-msg">{{ saveMsg }}</span>
    </div>

    </div><!-- /settings-body -->
  </div>
</template>

<style scoped>
.arxiv-settings { display: flex; flex-direction: column; overflow-y: auto; }
.settings-body { display: flex; flex-direction: column; gap: 14px; padding: 20px 24px; overflow-y: auto; flex: 1; }
.settings-header {
  display: flex; align-items: center;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  flex-shrink: 0;
  gap: 0;
}
.traffic-lights { display: flex; align-items: center; gap: 7px; flex-shrink: 0; }
.tl-btn {
  width: 13px; height: 13px; border-radius: 50%; border: none;
  cursor: default; display: flex; align-items: center; justify-content: center;
  transition: filter 0.1s; padding: 0; flex-shrink: 0;
}
.tl-red    { background: #FF5F57; cursor: pointer; }
.tl-yellow { background: #FFBD2E; }
.tl-green  { background: #28C840; }
.tl-btn:active { filter: brightness(0.85); }
.settings-title { flex: 1; text-align: center; font-size: 13px; font-weight: 600; color: var(--text-primary); }
.tl-spacer { width: 46px; flex-shrink: 0; }

.section-title { font-size: var(--font-size-lg); font-weight: 600; }
.section-desc { font-size: var(--font-size-sm); color: var(--text-secondary); }
.section-sep { font-size: 12px; font-weight: 600; letter-spacing: 0; color: var(--text-secondary); border-top: 1px solid var(--border-subtle); padding-top: 12px; }
.field-group { display: flex; flex-direction: column; gap: 6px; }
.field-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.highlight-row { background: color-mix(in srgb, var(--accent) 6%, transparent); border-radius: var(--radius-md); padding: 10px 12px; }
/* Source toggle block */
.source-toggles {
  display: flex; flex-direction: column; gap: 6px;
}
.source-toggle-row {
  display: flex; align-items: center; justify-content: space-between; gap: 12px;
  padding: 8px 12px; border-radius: var(--radius-md);
}
.arxiv-toggle-row  { background: color-mix(in srgb, #6366f1 6%, transparent); }
.biorxiv-toggle-row { background: color-mix(in srgb, #10b981 6%, transparent); }
.source-label { display: flex; align-items: center; gap: 7px; }
.source-pill {
  font-size: 10px; font-weight: 700; padding: 1px 6px; border-radius: 4px;
  letter-spacing: 0.3px;
}
.arxiv-pill-color   { background: #e0e7ff; color: #3730a3; }
.biorxiv-pill-color { background: #d1fae5; color: #065f46; }
.section-dimmed { opacity: 0.45; pointer-events: none; }
.label-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.field-label { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-primary); }
.field-hint { font-size: var(--font-size-xs); color: var(--text-tertiary); line-height: 1.4; }
.field-hint.warn { color: #f59e0b; }
.field-input { padding: 5px 8px; font-size: var(--font-size-sm); border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-primary); color: var(--text-primary); width: 100%; }
.field-input.xs { width: 72px; text-align: right; }
.field-input.sm { width: 100px; }
.field-textarea { padding: 6px 8px; font-size: var(--font-size-sm); border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-primary); color: var(--text-primary); width: 100%; resize: vertical; font-family: inherit; }
.prompt-textarea {
  min-height: 220px;
  line-height: 1.55;
  font-size: 12px;
  font-family: var(--font-mono);
  white-space: pre-wrap;
}
.threshold-control { display: inline-flex; align-items: center; gap: 6px; }
.concurrency-row { display: flex; align-items: center; gap: 8px; }
.concurrency-slider { flex: 1; cursor: pointer; }
.concurrency-value { font-size: 13px; font-weight: 700; min-width: 20px; text-align: right; color: var(--text-primary); }
.field-suffix { font-size: var(--font-size-xs); color: var(--text-tertiary); }
.cat-tags { display: flex; flex-wrap: wrap; gap: 6px; min-height: 24px; }
.cat-tag { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; padding: 2px 8px; background: var(--bg-secondary); border-radius: 10px; }
.cat-tag button { color: var(--text-tertiary); font-size: 12px; line-height: 1; }
.cat-presets { display: flex; flex-wrap: wrap; gap: 5px; align-items: center; }
.hint-label { font-size: var(--font-size-xs); color: var(--text-tertiary); }
.preset-btn { font-size: 11px; padding: 2px 7px; border: 1px solid var(--border-default); border-radius: 8px; color: var(--text-secondary); transition: all 0.1s; }
.preset-btn:hover { border-color: var(--accent); color: var(--accent); }
.preset-btn.active { background: color-mix(in srgb, var(--accent) 15%, transparent); color: var(--accent); border-color: var(--accent); }
.add-cat-row { display: flex; gap: 6px; }
.toggle { display: inline-flex; align-items: center; cursor: pointer; flex-shrink: 0; }
.toggle input { display: none; }
.toggle-track { width: 36px; height: 20px; background: var(--border-default); border-radius: 10px; position: relative; transition: background 0.15s; }
.toggle input:checked + .toggle-track { background: var(--accent); }
.toggle-track::after { content: ''; position: absolute; width: 14px; height: 14px; border-radius: 50%; background: #fff; top: 3px; left: 3px; transition: left 0.15s; }
.toggle input:checked + .toggle-track::after { left: 19px; }
.action-row { display: flex; align-items: center; gap: 10px; padding-top: 4px; }
.save-msg { font-size: var(--font-size-xs); color: var(--accent); }
.btn-primary { padding: 6px 14px; font-size: var(--font-size-sm); font-weight: 500; background: var(--accent); color: #fff; border-radius: var(--radius-sm); }
.btn-primary:hover:not(:disabled) { background: var(--accent-hover); }
.btn-primary:disabled { opacity: 0.4; cursor: not-allowed; }
.btn-ghost { padding: 5px 10px; font-size: var(--font-size-sm); color: var(--text-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); white-space: nowrap; }
.btn-ghost:hover { background: var(--bg-hover); }
.link-btn { font-size: var(--font-size-xs); color: var(--accent); padding: 2px 0; }
.link-btn:hover { color: var(--accent-hover); }
</style>
