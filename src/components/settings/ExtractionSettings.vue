<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '../../stores/settings'
import { useAiStore } from '../../stores/ai'

const { t } = useI18n()
const settingsStore = useSettingsStore()
const aiStore = useAiStore()

const DEFAULT_METADATA_AI_PROMPT = `Extract academic paper metadata from the text below.
Do NOT guess or infer missing fields — use null for anything not explicitly found in the text.
Reply with a JSON code block in exactly this format:

\`\`\`json
{"title": "...", "authors": ["First Last", "First Last"], "year": 2024, "venue": "...", "doi": "...", "arxiv_id": "..."}
\`\`\`

Text:
{text}`

const DEFAULT_AI_SUMMARY_PROMPT = '帮我用中文讲一下这篇论文，讲的越详细越好，我有这个领域的通用基础，但是没有这个小方向的基础。输出的时候只包含关于论文的讲解，不要包含寒暄的内容。开始时先用一段话总结这篇论文的核心内容。请用markdown形式输出你的讲解内容。'

const DEFAULT_ABSTRACT_AI_PROMPT = `请只从下面给定的论文原文片段中抽取作者原文的 Abstract/摘要段落。
要求：
- 只输出从原文中找到的摘要正文，不要生成、改写、翻译或概括
- 去掉“Abstract”“摘要”等标题词，只保留摘要正文
- 如果给定文本中没有明确的摘要段落，请只输出 NOT_FOUND
- 不要输出解释、Markdown、代码块或额外格式

论文原文片段：
{fulltext}`

const LEGACY_ABSTRACT_AI_PROMPT = '请根据以下论文全文，用中文写一段简洁的摘要（3-5句话），概括论文的研究问题、方法和主要结论。只输出摘要内容，不要包含任何其他说明。\n\n论文全文：\n{fulltext}'

const LEGACY_AI_SUMMARY_PROMPT_PLAIN_DETAIL = '帮我用中文讲一下这篇论文，讲的越详细越好，我有这个领域的通用基础，但是没有这个小方向的基础。输出的时候只包含关于论文的讲解，不要包含寒暄的内容。开始时先用一段话总结这篇论文的核心内容。'

const LEGACY_AI_SUMMARY_PROMPT_FULLTEXT_DETAIL = `你是一名严谨的研究助理。请只根据下面的论文全文，用中文写一份 Markdown 格式的 AI 总结。

目标读者是没有读过这篇论文的人；读完后应能完整理解这篇论文做了什么、为什么重要、怎么做、实验结果如何、有什么局限和可借鉴点。

要求：
- 不要编造全文中没有的信息；如果某项信息在论文全文中没有找到，请明确写“论文中未找到明确说明”
- 保留关键术语、方法名、数据集名和指标名
- 结论要具体，避免泛泛而谈
- 如果全文被截断，请基于可见内容总结，并说明可能不完整

请使用以下结构：
# AI 总结
## 一句话概括
## 研究背景与问题
## 核心创新点
- 列出 3-5 点，并解释每一点为什么重要
## 方法详解
- 详细说明模型、算法、系统设计、数据流程或理论推导
- 解释关键公式、模块或实验设计背后的直觉
## 实验设置
- 数据集、基线方法、评价指标、训练/实现细节
## 主要结果
- 总结最关键的定量和定性结果
- 对比基线，解释结果说明了什么
## 局限与注意点
## 适合引用的结论
## 延伸阅读或实践启发

论文全文：
{fulltext}`

const LEGACY_AI_SUMMARY_PROMPT = `你是一名严谨的研究助理。请根据下面的论文全文和元数据，用中文写一份 Markdown 格式的 AI 总结。

目标读者是没有读过这篇论文的人；读完后应能完整理解这篇论文做了什么、为什么重要、怎么做、实验结果如何、有什么局限和可借鉴点。不要编造文本中没有的信息；如果某项信息在论文中没有找到，请明确写“论文中未找到明确说明”。

请使用以下结构：
# AI 总结
## 一句话概括
## 研究背景与问题
## 核心创新点
- 列出 3-5 点，并解释每一点为什么重要
## 方法详解
- 详细说明模型、算法、系统设计、数据流程或理论推导
- 解释关键公式、模块或实验设计背后的直觉
## 实验设置
- 数据集、基线方法、评价指标、训练/实现细节
## 主要结果
- 总结最关键的定量和定性结果
- 对比基线，解释结果说明了什么
## 局限与注意点
## 适合引用的结论
## 延伸阅读或实践启发

论文元数据：
标题：{title}
作者：{authors}
年份：{year}
期刊/会议：{venue}
DOI：{doi}
arXiv：{arxiv_id}
摘要：{abstract}

论文全文：
{fulltext}`

onMounted(async () => {
  if (!aiStore.loaded) await aiStore.load()
})

const metadataPromptDraft = ref('')
const metadataPromptSaved = ref(false)
const summaryPromptDraft = ref('')
const summaryPromptSaved = ref(false)
const abstractPromptDraft = ref('')
const abstractPromptSaved = ref(false)
const translatePromptDraft = ref('')
const translatePromptSaved = ref(false)
const titlePromptDraft = ref('')
const titlePromptSaved = ref(false)

const DEFAULT_TRANSLATE_AI_PROMPT = '请将以下英文文本翻译成中文，保持学术风格，直接输出翻译结果，不需要任何额外说明：\n\n{text}'
const DEFAULT_TITLE_AI_PROMPT = '请根据以下对话内容生成一个简洁的标题（不超过20字，直接输出标题文字，不要引号和多余说明）：\n\n用户：{user_msg}\n\nAI：{ai_msg}'

function normalizeMetadataPrompt(prompt?: string) {
  const trimmed = prompt?.trim()
  return !trimmed
    || trimmed.includes('a concise paper abstract')
    || trimmed.includes('- abstract: string or null')
    || trimmed.includes('No markdown, no code blocks')
    || (trimmed.includes('Output ONLY the JSON object') && !trimmed.includes('```json'))
    ? DEFAULT_METADATA_AI_PROMPT
    : trimmed
}

function normalizeSummaryPrompt(prompt?: string) {
  const trimmed = prompt?.trim()
  if (
    !trimmed ||
    trimmed === LEGACY_AI_SUMMARY_PROMPT ||
    trimmed === LEGACY_AI_SUMMARY_PROMPT_FULLTEXT_DETAIL ||
    trimmed === LEGACY_AI_SUMMARY_PROMPT_PLAIN_DETAIL
  ) {
    return DEFAULT_AI_SUMMARY_PROMPT
  }
  return trimmed
}

function normalizeAbstractPrompt(prompt?: string) {
  const trimmed = prompt?.trim()
  return !trimmed || trimmed === LEGACY_ABSTRACT_AI_PROMPT
    ? DEFAULT_ABSTRACT_AI_PROMPT
    : trimmed
}

async function setMetaProvider(providerId: string) {
  const id = providerId || undefined
  await settingsStore.save({ metadata_ai_provider_id: id, metadata_ai_model_id: undefined })
}

async function setMetaModel(modelId: string) {
  await settingsStore.save({ metadata_ai_model_id: modelId || undefined })
}

const selectedProvider = computed(() =>
  aiStore.settings.providers.find(p => p.id === settingsStore.settings.metadata_ai_provider_id)
)

const availableModels = computed(() => selectedProvider.value?.models ?? [])

async function setMetadataPrompt(prompt: string) {
  await settingsStore.save({ metadata_ai_prompt: normalizeMetadataPrompt(prompt) })
  metadataPromptDraft.value = normalizeMetadataPrompt(settingsStore.settings.metadata_ai_prompt)
  metadataPromptSaved.value = true
  setTimeout(() => { metadataPromptSaved.value = false }, 1800)
}

async function resetMetadataPrompt() {
  await settingsStore.save({ metadata_ai_prompt: DEFAULT_METADATA_AI_PROMPT })
  metadataPromptDraft.value = DEFAULT_METADATA_AI_PROMPT
  metadataPromptSaved.value = true
  setTimeout(() => { metadataPromptSaved.value = false }, 1800)
}

async function setSummaryProvider(providerId: string) {
  const id = providerId || undefined
  await settingsStore.save({ ai_summary_provider_id: id, ai_summary_model_id: undefined })
}

async function setSummaryModel(modelId: string) {
  await settingsStore.save({ ai_summary_model_id: modelId || undefined })
}

async function setSummaryPrompt(prompt: string) {
  await settingsStore.save({ ai_summary_prompt: normalizeSummaryPrompt(prompt) })
  summaryPromptDraft.value = normalizeSummaryPrompt(settingsStore.settings.ai_summary_prompt)
  summaryPromptSaved.value = true
  setTimeout(() => { summaryPromptSaved.value = false }, 1800)
}

async function resetSummaryPrompt() {
  await settingsStore.save({ ai_summary_prompt: DEFAULT_AI_SUMMARY_PROMPT })
  summaryPromptDraft.value = DEFAULT_AI_SUMMARY_PROMPT
  summaryPromptSaved.value = true
  setTimeout(() => { summaryPromptSaved.value = false }, 1800)
}

const selectedSummaryProvider = computed(() =>
  aiStore.settings.providers.find(p => p.id === settingsStore.settings.ai_summary_provider_id)
)

const availableSummaryModels = computed(() => selectedSummaryProvider.value?.models ?? [])

async function setAbstractProvider(providerId: string) {
  const id = providerId || undefined
  await settingsStore.save({ abstract_ai_provider_id: id, abstract_ai_model_id: undefined })
}

async function setAbstractModel(modelId: string) {
  await settingsStore.save({ abstract_ai_model_id: modelId || undefined })
}

async function setAbstractPrompt(prompt: string) {
  await settingsStore.save({ abstract_ai_prompt: normalizeAbstractPrompt(prompt) })
  abstractPromptDraft.value = normalizeAbstractPrompt(settingsStore.settings.abstract_ai_prompt)
  abstractPromptSaved.value = true
  setTimeout(() => { abstractPromptSaved.value = false }, 1800)
}

async function resetAbstractPrompt() {
  await settingsStore.save({ abstract_ai_prompt: DEFAULT_ABSTRACT_AI_PROMPT })
  abstractPromptDraft.value = DEFAULT_ABSTRACT_AI_PROMPT
  abstractPromptSaved.value = true
  setTimeout(() => { abstractPromptSaved.value = false }, 1800)
}

const selectedAbstractProvider = computed(() =>
  aiStore.settings.providers.find(p => p.id === settingsStore.settings.abstract_ai_provider_id)
)

const availableAbstractModels = computed(() => selectedAbstractProvider.value?.models ?? [])

async function setTranslateProvider(providerId: string) {
  const id = providerId || undefined
  await settingsStore.save({ translate_ai_provider_id: id, translate_ai_model_id: undefined })
}

async function setTranslateModel(modelId: string) {
  await settingsStore.save({ translate_ai_model_id: modelId || undefined })
}

async function setTranslatePrompt(prompt: string) {
  const val = prompt.trim() || DEFAULT_TRANSLATE_AI_PROMPT
  await settingsStore.save({ translate_ai_prompt: val })
  translatePromptDraft.value = val
  translatePromptSaved.value = true
  setTimeout(() => { translatePromptSaved.value = false }, 1800)
}

async function resetTranslatePrompt() {
  await settingsStore.save({ translate_ai_prompt: DEFAULT_TRANSLATE_AI_PROMPT })
  translatePromptDraft.value = DEFAULT_TRANSLATE_AI_PROMPT
  translatePromptSaved.value = true
  setTimeout(() => { translatePromptSaved.value = false }, 1800)
}

const selectedTranslateProvider = computed(() =>
  aiStore.settings.providers.find(p => p.id === settingsStore.settings.translate_ai_provider_id)
)
const availableTranslateModels = computed(() => selectedTranslateProvider.value?.models ?? [])

async function setTitleProvider(providerId: string) {
  const id = providerId || undefined
  await settingsStore.save({ title_ai_provider_id: id, title_ai_model_id: undefined })
}

async function setTitleModel(modelId: string) {
  await settingsStore.save({ title_ai_model_id: modelId || undefined })
}

async function setTitlePrompt(prompt: string) {
  const val = prompt.trim() || DEFAULT_TITLE_AI_PROMPT
  await settingsStore.save({ title_ai_prompt: val })
  titlePromptDraft.value = val
  titlePromptSaved.value = true
  setTimeout(() => { titlePromptSaved.value = false }, 1800)
}

async function resetTitlePrompt() {
  await settingsStore.save({ title_ai_prompt: DEFAULT_TITLE_AI_PROMPT })
  titlePromptDraft.value = DEFAULT_TITLE_AI_PROMPT
  titlePromptSaved.value = true
  setTimeout(() => { titlePromptSaved.value = false }, 1800)
}

const selectedTitleProvider = computed(() =>
  aiStore.settings.providers.find(p => p.id === settingsStore.settings.title_ai_provider_id)
)
const availableTitleModels = computed(() => selectedTitleProvider.value?.models ?? [])

const titlePromptValue = computed(() =>
  (settingsStore.settings.title_ai_prompt?.trim() || DEFAULT_TITLE_AI_PROMPT)
)

const metadataPromptValue = computed(() =>
  normalizeMetadataPrompt(settingsStore.settings.metadata_ai_prompt)
)

const summaryPromptValue = computed(() =>
  normalizeSummaryPrompt(settingsStore.settings.ai_summary_prompt)
)

const abstractPromptValue = computed(() =>
  normalizeAbstractPrompt(settingsStore.settings.abstract_ai_prompt)
)

watch(
  metadataPromptValue,
  (value) => {
    if (metadataPromptDraft.value !== value) {
      metadataPromptDraft.value = value
    }
  },
  { immediate: true }
)

watch(
  summaryPromptValue,
  (value) => {
    if (summaryPromptDraft.value !== value) {
      summaryPromptDraft.value = value
    }
  },
  { immediate: true }
)

watch(
  abstractPromptValue,
  (value) => {
    if (abstractPromptDraft.value !== value) {
      abstractPromptDraft.value = value
    }
  },
  { immediate: true }
)

const translatePromptValue = computed(() =>
  (settingsStore.settings.translate_ai_prompt?.trim() || DEFAULT_TRANSLATE_AI_PROMPT)
)

watch(
  translatePromptValue,
  (value) => {
    if (translatePromptDraft.value !== value) translatePromptDraft.value = value
  },
  { immediate: true }
)

let metaPromptTimer: ReturnType<typeof setTimeout> | null = null
let summaryPromptTimer: ReturnType<typeof setTimeout> | null = null
let abstractPromptTimer: ReturnType<typeof setTimeout> | null = null
let translatePromptTimer: ReturnType<typeof setTimeout> | null = null
let titlePromptTimer: ReturnType<typeof setTimeout> | null = null

watch(
  titlePromptValue,
  (value) => {
    if (titlePromptDraft.value !== value) titlePromptDraft.value = value
  },
  { immediate: true }
)

watch(titlePromptDraft, (val) => {
  if (val === titlePromptValue.value) return
  if (titlePromptTimer) clearTimeout(titlePromptTimer)
  titlePromptTimer = setTimeout(() => setTitlePrompt(val), 800)
})

watch(metadataPromptDraft, (val) => {
  if (val === metadataPromptValue.value) return
  if (metaPromptTimer) clearTimeout(metaPromptTimer)
  metaPromptTimer = setTimeout(() => setMetadataPrompt(val), 800)
})

watch(summaryPromptDraft, (val) => {
  if (val === summaryPromptValue.value) return
  if (summaryPromptTimer) clearTimeout(summaryPromptTimer)
  summaryPromptTimer = setTimeout(() => setSummaryPrompt(val), 800)
})

watch(abstractPromptDraft, (val) => {
  if (val === abstractPromptValue.value) return
  if (abstractPromptTimer) clearTimeout(abstractPromptTimer)
  abstractPromptTimer = setTimeout(() => setAbstractPrompt(val), 800)
})

watch(translatePromptDraft, (val) => {
  if (val === translatePromptValue.value) return
  if (translatePromptTimer) clearTimeout(translatePromptTimer)
  translatePromptTimer = setTimeout(() => setTranslatePrompt(val), 800)
})

</script>

<template>
  <div class="settings-panel">
    <h2>{{ t('settings.extraction') }}</h2>
    <p class="desc">{{ t('settings.extractionDesc') }}</p>

    <!-- Translate model and prompt -->
    <div class="setting-group">
      <div class="setting-label">{{ t('settings.translateAiSection') }}</div>
      <div class="setting-hint" style="margin-bottom: 10px">{{ t('settings.translateAiDesc') }}</div>

      <div v-if="aiStore.settings.providers.length === 0" class="no-providers">
        {{ t('settings.metaAiNoProviders') }}
      </div>
      <template v-else>
        <div class="field-row">
          <label class="field-label">{{ t('settings.metaAiProvider') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.translate_ai_provider_id ?? ''"
            @change="setTranslateProvider(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="p in aiStore.settings.providers.filter(p => p.enabled)"
              :key="p.id"
              :value="p.id"
            >{{ p.name }}</option>
          </select>
        </div>

        <div v-if="selectedTranslateProvider && availableTranslateModels.length > 0" class="field-row">
          <label class="field-label">{{ t('settings.metaAiModel') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.translate_ai_model_id ?? ''"
            @change="setTranslateModel(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="m in availableTranslateModels"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <div class="prompt-head">
        <div>
          <label class="field-label prompt-label">{{ t('settings.translatePrompt') }}</label>
          <div class="setting-hint">{{ t('settings.translatePromptHint') }}</div>
        </div>
        <div class="prompt-actions">
          <span v-if="translatePromptSaved" class="saved-pill">{{ t('settings.saved') }}</span>
          <button class="text-btn" @click="resetTranslatePrompt">{{ t('settings.translatePromptReset') }}</button>
          <button class="prompt-save-btn" @click="setTranslatePrompt(translatePromptDraft)">
            {{ t('settings.save') }}
          </button>
        </div>
      </div>
      <textarea
        class="prompt-textarea translate-prompt"
        v-model="translatePromptDraft"
        spellcheck="false"
        @blur="setTranslatePrompt(translatePromptDraft)"
      />
    </div>

    <!-- Chat title generation -->
    <div class="setting-group">
      <div class="setting-label">{{ t('settings.titleAiSection') }}</div>
      <div class="setting-hint" style="margin-bottom: 10px">{{ t('settings.titleAiDesc') }}</div>

      <div v-if="aiStore.settings.providers.length === 0" class="no-providers">
        {{ t('settings.metaAiNoProviders') }}
      </div>
      <template v-else>
        <div class="field-row">
          <label class="field-label">{{ t('settings.metaAiProvider') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.title_ai_provider_id ?? ''"
            @change="setTitleProvider(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="p in aiStore.settings.providers.filter(p => p.enabled)"
              :key="p.id"
              :value="p.id"
            >{{ p.name }}</option>
          </select>
        </div>

        <div v-if="selectedTitleProvider && availableTitleModels.length > 0" class="field-row">
          <label class="field-label">{{ t('settings.metaAiModel') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.title_ai_model_id ?? ''"
            @change="setTitleModel(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="m in availableTitleModels"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <div class="prompt-head">
        <div>
          <label class="field-label prompt-label">{{ t('settings.titlePrompt') }}</label>
          <div class="setting-hint">{{ t('settings.titlePromptHint') }}</div>
        </div>
        <div class="prompt-actions">
          <span v-if="titlePromptSaved" class="saved-pill">{{ t('settings.saved') }}</span>
          <button class="text-btn" @click="resetTitlePrompt">{{ t('settings.titlePromptReset') }}</button>
          <button class="prompt-save-btn" @click="setTitlePrompt(titlePromptDraft)">
            {{ t('settings.save') }}
          </button>
        </div>
      </div>
      <textarea
        class="prompt-textarea"
        v-model="titlePromptDraft"
        spellcheck="false"
        @blur="setTitlePrompt(titlePromptDraft)"
      />
    </div>

    <!-- Metadata AI model -->
    <div class="setting-group">
      <div class="setting-label">{{ t('settings.metaAiSection') }}</div>
      <div class="setting-hint" style="margin-bottom: 10px">{{ t('settings.metaAiDesc') }}</div>

      <div v-if="aiStore.settings.providers.length === 0" class="no-providers">
        {{ t('settings.metaAiNoProviders') }}
      </div>
      <template v-else>
        <div class="field-row">
          <label class="field-label">{{ t('settings.metaAiProvider') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.metadata_ai_provider_id ?? ''"
            @change="setMetaProvider(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="p in aiStore.settings.providers.filter(p => p.enabled)"
              :key="p.id"
              :value="p.id"
            >{{ p.name }}</option>
          </select>
        </div>

        <div v-if="selectedProvider && availableModels.length > 0" class="field-row">
          <label class="field-label">{{ t('settings.metaAiModel') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.metadata_ai_model_id ?? ''"
            @change="setMetaModel(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="m in availableModels"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <div class="prompt-head">
        <div>
          <label class="field-label prompt-label">{{ t('settings.metaPrompt') }}</label>
          <div class="setting-hint">{{ t('settings.metaPromptHint') }}</div>
        </div>
        <div class="prompt-actions">
          <span v-if="metadataPromptSaved" class="saved-pill">{{ t('settings.saved') }}</span>
          <button class="text-btn" @click="resetMetadataPrompt">{{ t('settings.summaryPromptReset') }}</button>
          <button class="prompt-save-btn" @click="setMetadataPrompt(metadataPromptDraft)">
            {{ t('settings.save') }}
          </button>
        </div>
      </div>
      <textarea
        class="prompt-textarea metadata-prompt"
        v-model="metadataPromptDraft"
        spellcheck="false"
        @blur="setMetadataPrompt(metadataPromptDraft)"
      />
    </div>

    <!-- AI abstract model and prompt -->
    <div class="setting-group">
      <div class="setting-label">{{ t('settings.abstractAiSection') }}</div>
      <div class="setting-hint" style="margin-bottom: 10px">{{ t('settings.abstractAiDesc') }}</div>

      <div v-if="aiStore.settings.providers.length === 0" class="no-providers">
        {{ t('settings.metaAiNoProviders') }}
      </div>
      <template v-else>
        <div class="field-row">
          <label class="field-label">{{ t('settings.metaAiProvider') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.abstract_ai_provider_id ?? ''"
            @change="setAbstractProvider(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="p in aiStore.settings.providers.filter(p => p.enabled)"
              :key="p.id"
              :value="p.id"
            >{{ p.name }}</option>
          </select>
        </div>

        <div v-if="selectedAbstractProvider && availableAbstractModels.length > 0" class="field-row">
          <label class="field-label">{{ t('settings.metaAiModel') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.abstract_ai_model_id ?? ''"
            @change="setAbstractModel(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="m in availableAbstractModels"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <div class="prompt-head">
        <div>
          <label class="field-label prompt-label">{{ t('settings.abstractPrompt') }}</label>
          <div class="setting-hint">{{ t('settings.abstractPromptHint') }}</div>
        </div>
        <div class="prompt-actions">
          <span v-if="abstractPromptSaved" class="saved-pill">{{ t('settings.saved') }}</span>
          <button class="text-btn" @click="resetAbstractPrompt">{{ t('settings.abstractPromptReset') }}</button>
          <button class="prompt-save-btn" @click="setAbstractPrompt(abstractPromptDraft)">
            {{ t('settings.save') }}
          </button>
        </div>
      </div>
      <textarea
        class="prompt-textarea abstract-prompt"
        v-model="abstractPromptDraft"
        spellcheck="false"
        @blur="setAbstractPrompt(abstractPromptDraft)"
      />
    </div>

    <!-- AI summary model and prompt -->
    <div class="setting-group">
      <div class="setting-label">{{ t('settings.summaryAiSection') }}</div>
      <div class="setting-hint" style="margin-bottom: 10px">{{ t('settings.summaryAiDesc') }}</div>

      <div v-if="aiStore.settings.providers.length === 0" class="no-providers">
        {{ t('settings.metaAiNoProviders') }}
      </div>
      <template v-else>
        <div class="field-row">
          <label class="field-label">{{ t('settings.metaAiProvider') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.ai_summary_provider_id ?? ''"
            @change="setSummaryProvider(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="p in aiStore.settings.providers.filter(p => p.enabled)"
              :key="p.id"
              :value="p.id"
            >{{ p.name }}</option>
          </select>
        </div>

        <div v-if="selectedSummaryProvider && availableSummaryModels.length > 0" class="field-row">
          <label class="field-label">{{ t('settings.metaAiModel') }}</label>
          <select
            class="field-select"
            :value="settingsStore.settings.ai_summary_model_id ?? ''"
            @change="setSummaryModel(($event.target as HTMLSelectElement).value)"
          >
            <option value="">{{ t('settings.metaAiDefault') }}</option>
            <option
              v-for="m in availableSummaryModels"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name || m.id }}</option>
          </select>
        </div>
      </template>

      <div class="prompt-head">
        <div>
          <label class="field-label prompt-label">{{ t('settings.summaryPrompt') }}</label>
          <div class="setting-hint">{{ t('settings.summaryPromptHint') }}</div>
        </div>
        <div class="prompt-actions">
          <span v-if="summaryPromptSaved" class="saved-pill">{{ t('settings.saved') }}</span>
          <button class="text-btn" @click="resetSummaryPrompt">{{ t('settings.summaryPromptReset') }}</button>
          <button class="prompt-save-btn" @click="setSummaryPrompt(summaryPromptDraft)">
            {{ t('settings.save') }}
          </button>
        </div>
      </div>
      <textarea
        class="prompt-textarea"
        v-model="summaryPromptDraft"
        spellcheck="false"
        @blur="setSummaryPrompt(summaryPromptDraft)"
      />
    </div>

  </div>
</template>

<style scoped>
.settings-panel { padding: 24px 28px; }
h2 { font-size: 18px; font-weight: 600; margin-bottom: 8px; color: var(--text-primary); }
.desc { font-size: var(--font-size-sm); color: var(--text-secondary); margin-bottom: 24px; line-height: 1.5; }

.setting-group { margin-bottom: 24px; }
.setting-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 4px;
  letter-spacing: 0;
}
.setting-hint { font-size: var(--font-size-xs); color: var(--text-tertiary); line-height: 1.4; }

.no-providers {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  line-height: 1.45;
}

.field-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
}
.field-label {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  width: 80px;
  flex-shrink: 0;
}
.field-select {
  flex: 1;
  padding: 5px 8px;
  font-size: var(--font-size-sm);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
}

.prompt-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 12px;
  margin-bottom: 8px;
}
.prompt-label {
  width: auto;
  margin: 0 0 3px;
  color: var(--text-secondary);
}
.prompt-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
.saved-pill {
  font-size: var(--font-size-xs);
  color: #15803d;
  background: color-mix(in srgb, #22c55e 14%, transparent);
  border: 1px solid color-mix(in srgb, #22c55e 22%, transparent);
  border-radius: var(--radius-pill);
  padding: 3px 8px;
}
.text-btn {
  font-size: var(--font-size-xs);
  color: var(--accent);
  padding: 3px 6px;
  border-radius: var(--radius-sm);
}
.text-btn:hover { background: color-mix(in srgb, var(--accent) 10%, transparent); }
.prompt-save-btn {
  height: 26px;
  padding: 0 11px;
  border-radius: var(--radius-pill);
  background: var(--accent);
  color: #fff;
  font-size: var(--font-size-xs);
  font-weight: 600;
  white-space: nowrap;
}
.prompt-save-btn:hover { opacity: 0.86; }
.prompt-textarea {
  width: 100%;
  min-height: 280px;
  resize: vertical;
  padding: 10px 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-default);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.55;
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
  outline: none;
}
.metadata-prompt {
  min-height: 210px;
}
.abstract-prompt {
  min-height: 160px;
}
.translate-prompt {
  min-height: 100px;
}
.prompt-textarea:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 12%, transparent);
}
.prompt-hint { margin-top: 6px; }

.btn-secondary {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 14px;
  font-size: var(--font-size-sm);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.12s;
}
.btn-secondary:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
.btn-secondary:disabled { opacity: 0.6; cursor: not-allowed; }

@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
.spin { animation: spin 0.7s linear infinite; }
</style>
