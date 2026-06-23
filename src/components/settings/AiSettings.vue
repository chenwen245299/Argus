<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useAiStore } from '../../stores/ai'
import { useSettingsStore } from '../../stores/settings'
import type { AiModel, AiProviderInfo, AiProviderInput, ModelSelection } from '../../types'

const { t } = useI18n()
const ai = useAiStore()
const settingsStore = useSettingsStore()

// ── Presets ───────────────────────────────────────────────────────────────────

const PRESETS = [
  { label: 'OpenAI',       base_url: 'https://api.openai.com/v1',         kind: 'openai_compatible' },
  { label: 'OpenRouter',   base_url: 'https://openrouter.ai/api/v1',      kind: 'openrouter' },
  { label: 'DeepSeek',     base_url: 'https://api.deepseek.com/v1',       kind: 'openai_compatible' },
  { label: 'Kimi Code',    base_url: 'https://api.kimi.com/coding/v1',    kind: 'kimi' },
  { label: 'Ollama (local)',base_url:'http://localhost:11434/v1',          kind: 'openai_compatible' },
  { label: 'Anthropic',    base_url: 'https://api.anthropic.com/v1',      kind: 'anthropic' },
]

const CAPABILITY_OPTIONS = [
  { id: 'reasoning', labelKey: 'aiService.capReasoning' },
  { id: 'vision', labelKey: 'aiService.capVision' },
  { id: 'tool_calling', labelKey: 'aiService.capToolCalling' },
  { id: 'embedding', labelKey: 'aiService.capEmbedding' },
]

const CAPABILITY_LABEL_ALIASES: Record<string, string> = {
  function_calling: 'aiService.capFunctionCalling',
}

const FETCH_GROUP_ORDER = ['embedding', 'vision', 'tool_calling', 'reasoning', 'other'] as const
type FetchGroupId = typeof FETCH_GROUP_ORDER[number]

type ModelForm = {
  id: string
  display_name: string
  context_length: string
  capabilities: string[]
  input_price: string   // CNY per 1M input tokens, empty = not set
  output_price: string  // CNY per 1M output tokens, empty = not set
  provider_order: string[] // OpenRouter provider preference order
}

interface OrEndpoint {
  name: string
  quantization: string | null
  context_length?: number
}

function emptyModelForm(): ModelForm {
  return { id: '', display_name: '', context_length: '', capabilities: [], input_price: '', output_price: '', provider_order: [] }
}

// ── State ─────────────────────────────────────────────────────────────────────

const selectedId    = ref<string | null>(null)
const isAdding      = ref(false)

// Add-provider form
const addForm = ref({ name: '', base_url: '', kind: 'openai_compatible', enabled: true, api_key: '' })

// Edit state for the selected provider (local copy, saved on submit)
const editKey       = ref('')           // new key (blank = no change)
const editKeyMode   = ref(false)        // show key input
const editName      = ref('')
const editUrl       = ref('')
const editKind      = ref('')
const editEnabled   = ref(true)
const editModels    = ref<AiModel[]>([])

// Test connection
const testStatus    = ref<'' | 'testing' | 'ok' | 'fail'>('')
const testMsg       = ref('')

// Fetch models / selection dialog
const fetchStatus      = ref<'' | 'fetching'>('')
const fetchErr         = ref('')
const showFetchDialog  = ref(false)
const fetchedModels    = ref<AiModel[]>([])
const fetchSelected    = ref<Set<string>>(new Set())
const fetchSearch      = ref('')
const fetchCapability  = ref<'all' | FetchGroupId>('all')

// Add manual model form
const showAddModel  = ref(false)
const newModel      = ref<ModelForm>(emptyModelForm())

// Edit model inline
const editModelIdx  = ref<number | null>(null)
const editModelForm = ref<ModelForm>(emptyModelForm())

// OpenRouter endpoint fetch (per model being edited)
const orEndpoints       = ref<OrEndpoint[]>([])
const orEndpointStatus  = ref<'' | 'fetching' | 'ok' | 'fail'>('')
const orEndpointErr     = ref('')

// Default model selection
const defaultSel    = ref<ModelSelection | null>(null)

// Save feedback
const saveStatus    = ref<'' | 'saving' | 'saved'>('')
const providerCtxMenu = ref<{ x: number; y: number; provider: AiProviderInfo } | null>(null)

onMounted(async () => {
  await Promise.all([ai.load(), settingsStore.load()])
  defaultSel.value = ai.defaultSelection
  if (ai.settings.providers.length > 0) {
    selectProvider(ai.settings.providers[0].id)
  }
})

onMounted(() => {
  document.addEventListener('pointerdown', onGlobalPointerDown, true)
  document.addEventListener('keydown', onGlobalKeydown)
})

onUnmounted(() => {
  document.removeEventListener('pointerdown', onGlobalPointerDown, true)
  document.removeEventListener('keydown', onGlobalKeydown)
})

const selectedProvider = computed(() =>
  ai.settings.providers.find(p => p.id === selectedId.value) ?? null
)

const fetchFilteredModels = computed(() => {
  const query = fetchSearch.value.trim().toLowerCase()
  return fetchedModels.value.filter(model => {
    const matchesSearch = !query || fetchModelSearchText(model).includes(query)
    const selectedCap = fetchCapability.value
    const matchesCapability = selectedCap === 'all' || hasFetchCapability(model, selectedCap)
    return matchesSearch && matchesCapability
  })
})

const fetchGroupedModels = computed(() => {
  const groups = new Map<FetchGroupId, AiModel[]>()
  for (const model of fetchFilteredModels.value) {
    const selectedCap = fetchCapability.value
    const groupId = selectedCap === 'all' ? primaryFetchCapability(model) : selectedCap
    if (!groups.has(groupId)) groups.set(groupId, [])
    groups.get(groupId)!.push(model)
  }
  return FETCH_GROUP_ORDER
    .map(id => ({ id, label: fetchGroupLabel(id), models: groups.get(id) ?? [] }))
    .filter(group => group.models.length > 0)
})

const fetchAvailableVisibleCount = computed(() => {
  const existingIds = new Set(editModels.value.map(m => m.id))
  return fetchFilteredModels.value.filter(m => !existingIds.has(m.id)).length
})

function selectProvider(id: string) {
  closeProviderCtx()
  isAdding.value = false
  selectedId.value = id
  const p = ai.settings.providers.find(x => x.id === id)
  if (!p) return
  editKey.value = ''
  editKeyMode.value = false
  editName.value = p.name
  editUrl.value = p.base_url
  editKind.value = p.kind
  editEnabled.value = p.enabled
  editModels.value = p.models.map(m => ({ ...m, enabled: true }))
  testStatus.value = ''
  testMsg.value = ''
  fetchErr.value = ''
  showAddModel.value = false
  editModelIdx.value = null
}

watch(() => ai.settings.providers, (providers) => {
  if (selectedId.value && !providers.find(p => p.id === selectedId.value)) {
    selectedId.value = providers[0]?.id ?? null
    if (selectedId.value) selectProvider(selectedId.value)
  }
}, { deep: true })

// ── Add provider ──────────────────────────────────────────────────────────────

function startAdd() {
  isAdding.value = true
  selectedId.value = null
  addForm.value = { name: '', base_url: '', kind: 'openai_compatible', enabled: true, api_key: '' }
}

function applyPreset(preset: typeof PRESETS[0]) {
  addForm.value.base_url = preset.base_url
  addForm.value.kind = preset.kind
  if (!addForm.value.name) addForm.value.name = preset.label
}

async function submitAdd() {
  if (!addForm.value.name || !addForm.value.base_url) return
  const input: AiProviderInput = {
    name: addForm.value.name,
    base_url: addForm.value.base_url,
    kind: addForm.value.kind,
    enabled: true,
    models: [],
  }
  try {
    const p = await ai.addProvider(input, addForm.value.api_key)
    isAdding.value = false
    selectProvider(p.id)
  } catch (e) {
    alert(String(e))
  }
}

// ── Save provider details ─────────────────────────────────────────────────────

async function saveProvider() {
  if (!selectedId.value) return
  saveStatus.value = 'saving'
  const input: AiProviderInput = {
    id: selectedId.value,
    name: editName.value,
    base_url: editUrl.value,
    kind: editKind.value,
    enabled: editEnabled.value,
    models: editModels.value.map(m => ({ ...m, enabled: true })),
  }
  try {
    await ai.updateProvider(input, editKey.value || undefined)
    editKey.value = ''
    editKeyMode.value = false
    saveStatus.value = 'saved'
    setTimeout(() => { saveStatus.value = '' }, 2000)
  } catch (e) {
    saveStatus.value = ''
    alert(String(e))
  }
}

async function toggleEnabled() {
  if (!selectedId.value) return
  editEnabled.value = !editEnabled.value
  await ai.setProviderEnabled(selectedId.value, editEnabled.value)
}

async function toggleProviderEnabled(provider: AiProviderInfo) {
  closeProviderCtx()
  const next = !provider.enabled
  await ai.setProviderEnabled(provider.id, next)
  if (selectedId.value === provider.id) editEnabled.value = next
}

async function deleteProvider() {
  if (!selectedId.value || !selectedProvider.value) return
  await deleteProviderById(selectedProvider.value)
}

async function deleteProviderById(provider: AiProviderInfo) {
  const msg = t('aiService.deleteConfirm').replace('{name}', provider.name)
  if (!confirm(msg)) return
  closeProviderCtx()
  await ai.deleteProvider(provider.id)
  selectedId.value = ai.settings.providers[0]?.id ?? null
  if (selectedId.value) selectProvider(selectedId.value)
  else { isAdding.value = false }
}

function openProviderCtx(e: MouseEvent, provider: AiProviderInfo) {
  e.preventDefault()
  selectProvider(provider.id)
  providerCtxMenu.value = { x: e.clientX, y: e.clientY, provider }
}

function closeProviderCtx() {
  providerCtxMenu.value = null
}

function onGlobalPointerDown(e: PointerEvent) {
  if (!providerCtxMenu.value) return
  const target = e.target as HTMLElement | null
  if (target?.closest('.provider-ctx-menu')) return
  closeProviderCtx()
}

function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') closeProviderCtx()
}

// ── Test connection ───────────────────────────────────────────────────────────

async function testConnection() {
  if (!selectedId.value) return
  testStatus.value = 'testing'
  testMsg.value = ''
  try {
    testMsg.value = await ai.testProvider(selectedId.value)
    testStatus.value = 'ok'
  } catch (e) {
    testMsg.value = String(e)
    testStatus.value = 'fail'
  }
}

// ── Fetch models ──────────────────────────────────────────────────────────────

async function fetchModels() {
  if (!selectedId.value) return
  fetchStatus.value = 'fetching'
  fetchErr.value = ''
  fetchSearch.value = ''
  fetchCapability.value = 'all'
  try {
    const fetched = await ai.fetchModels(selectedId.value)
    fetchedModels.value = fetched
    fetchSelected.value = new Set()
    fetchStatus.value = ''
    showFetchDialog.value = true
  } catch (e) {
    fetchErr.value = String(e)
    fetchStatus.value = ''
    fetchedModels.value = []
    fetchSelected.value = new Set()
    showFetchDialog.value = true
  }
}

function toggleFetchSelect(id: string) {
  if (fetchSelected.value.has(id)) fetchSelected.value.delete(id)
  else fetchSelected.value.add(id)
  fetchSelected.value = new Set(fetchSelected.value)
}

function selectAllFetched() {
  const existingIds = new Set(editModels.value.map(m => m.id))
  const next = new Set(fetchSelected.value)
  for (const model of fetchFilteredModels.value) {
    if (!existingIds.has(model.id)) next.add(model.id)
  }
  fetchSelected.value = next
}

function deselectAllFetched() {
  fetchSelected.value = new Set()
}

function confirmFetchedSelection() {
  const existingIds = new Set(editModels.value.map(m => m.id))
  for (const m of fetchedModels.value) {
    if (fetchSelected.value.has(m.id) && !existingIds.has(m.id)) {
      editModels.value.push({ ...m, enabled: true })
    }
  }
  showFetchDialog.value = false
  saveProvider()
}

// ── Manual model add ──────────────────────────────────────────────────────────

function parsePrice(s: string): number | undefined {
  const v = parseFloat(s)
  return isNaN(v) || v < 0 ? undefined : v
}

function submitAddModel() {
  const { id, display_name, context_length, capabilities, input_price, output_price, provider_order } = newModel.value
  if (!id) return
  editModels.value.push({
    id,
    display_name: display_name || id,
    context_length: parseContextLength(context_length),
    capabilities: [...capabilities],
    enabled: true,
    input_price_per_million: parsePrice(input_price),
    output_price_per_million: parsePrice(output_price),
    provider_order: [...provider_order],
  })
  newModel.value = emptyModelForm()
  showAddModel.value = false
}

function startEditModel(idx: number) {
  editModelIdx.value = idx
  const m = editModels.value[idx]
  editModelForm.value = {
    id: m.id,
    display_name: m.display_name,
    context_length: m.context_length?.toString() ?? '',
    capabilities: [...m.capabilities],
    input_price: m.input_price_per_million?.toString() ?? '',
    output_price: m.output_price_per_million?.toString() ?? '',
    provider_order: [...(m.provider_order ?? [])],
  }
  if (isOpenRouterProvider.value && orEndpoints.value.length === 0 && orEndpointStatus.value !== 'fetching') {
    fetchOrEndpoints(m.id)
  }
}

function submitEditModel() {
  if (editModelIdx.value === null) return
  const { id, display_name, context_length, capabilities, input_price, output_price, provider_order } = editModelForm.value
  editModels.value[editModelIdx.value] = {
    ...editModels.value[editModelIdx.value],
    id,
    display_name: display_name || id,
    context_length: parseContextLength(context_length),
    capabilities: [...capabilities],
    enabled: true,
    input_price_per_million: parsePrice(input_price),
    output_price_per_million: parsePrice(output_price),
    provider_order: [...provider_order],
  }
  editModelIdx.value = null
}

async function saveEditModel() {
  submitEditModel()
  await saveProvider()
}

function removeModel(idx: number) { editModels.value.splice(idx, 1) }

// ── Billing settings ─────────────────────────────────────────────────────────

function currentUsdToCnyRate() {
  const rate = Number(settingsStore.settings.usd_to_cny_rate)
  return Number.isFinite(rate) && rate > 0 ? rate : 7.2
}

function priceCnyPerMillion(usd?: number, cny?: number) {
  if (usd != null) return usd * currentUsdToCnyRate()
  return cny
}

function fmtPricePerMillion(price?: number) {
  if (price == null) return ''
  if (price < 0.01) return '<¥0.01/M'
  return `¥${price.toFixed(price < 1 ? 3 : 2)}/M`
}

// ── OpenRouter provider selection ─────────────────────────────────────────────

const isOpenRouterProvider = computed(() => editKind.value === 'openrouter')

async function fetchOrEndpoints(modelId: string) {
  if (!selectedId.value || !modelId.trim()) return
  orEndpointStatus.value = 'fetching'
  orEndpointErr.value = ''
  orEndpoints.value = []
  try {
    const raw = await invoke<Record<string, unknown>>('fetch_openrouter_endpoints', {
      providerId: selectedId.value,
      modelId: modelId.trim(),
    })
    // Response: { data: { endpoints: [...] } }
    const eps = (raw?.data as Record<string, unknown>)?.endpoints as unknown[] | undefined
    if (Array.isArray(eps) && eps.length > 0) {
      orEndpoints.value = eps.map((e: unknown) => {
        const ep = e as Record<string, unknown>
        return {
          name: String(ep.name ?? ep.provider_name ?? ''),
          quantization: ep.quantization ? String(ep.quantization) : null,
          context_length: ep.context_length as number | undefined,
        }
      }).filter(ep => ep.name)
      orEndpointStatus.value = 'ok'
    } else {
      orEndpointStatus.value = 'fail'
      orEndpointErr.value = '未找到 provider 数据，请手动输入'
    }
  } catch (e) {
    orEndpointStatus.value = 'fail'
    orEndpointErr.value = `获取失败，请手动输入（${e}）`
  }
}

function toggleOrProvider(name: string, form: ModelForm) {
  const idx = form.provider_order.indexOf(name)
  if (idx >= 0) {
    form.provider_order.splice(idx, 1)
  } else {
    form.provider_order.push(name)
  }
}

// ── Default model ─────────────────────────────────────────────────────────────

async function setDefaultModel(providerId: string, modelId: string) {
  await ai.setDefault({ providerId, modelId })
  defaultSel.value = ai.defaultSelection
}

// ── Provider logo ─────────────────────────────────────────────────────────────

const LOGO_MAP: [string[], string][] = [
  [['deepseek', 'api.deepseek.com'], 'deepseek.svg'],
  [['openai', 'api.openai.com'], 'openai.svg'],
  [['anthropic', 'claude', 'api.anthropic.com'], 'claude.svg'],
  [['openrouter', 'openrouter.ai'], 'openrouter.svg'],
  [['kimi', 'moonshot'], 'kimi.svg'],
  [['ollama', '11434'], 'ollama-color.svg'],
  [['gemini', 'generativelanguage.googleapis.com'], 'gemini.svg'],
  [['gemma'], 'gemma.svg'],
  [['grok'], 'grok.svg'],
  [['xai', 'x.ai', 'api.x.ai'], 'xai.svg'],
  [['huggingface', 'huggingface.co'], 'huggingface.svg'],
  [['lmstudio'], 'lmstudio.svg'],
  [['siliconflow', 'silicon'], 'siliconflow.svg'],
  [['alibaba', 'qwen', 'dashscope', 'aliyun'], 'alibaba.svg'],
  [['baidu', 'qianfan', 'baidubce'], 'baidu.svg'],
  [['zhipu', 'bigmodel', 'chatglm'], 'zhipu.svg'],
  [['tencent', 'hunyuan'], 'tencent.svg'],
  [['bytedance', 'doubao', 'volcengine', 'volces'], 'bytedance.svg'],
  [['nvidia', 'integrate.api.nvidia'], 'nvidia.svg'],
  [['microsoft', 'azure', 'openai.azure'], 'microsoft.svg'],
  [['xiaomi', 'micloud'], 'xiaomi.svg'],
  [['mole', 'moleapi'], 'MoleAPI.svg'],
]

function providerLogoUrl(name: string, baseUrl: string): string | null {
  const haystack = `${name} ${baseUrl}`.toLowerCase()
  for (const [keywords, file] of LOGO_MAP) {
    if (keywords.some(k => haystack.includes(k))) {
      return new URL(`../../assets/providers/${file}`, import.meta.url).href
    }
  }
  return null
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function formatCtx(n: number | undefined): string {
  if (!n) return ''
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)}M`
  if (n >= 1_000) return `${Math.round(n / 1_000)}K`
  return String(n)
}

function parseContextLength(raw: string): number | undefined {
  const value = raw.trim()
  if (!value) return undefined
  const match = value.match(/^(\d+(?:\.\d+)?)\s*([km])?$/i)
  if (!match) return undefined
  const amount = Number(match[1])
  if (!Number.isFinite(amount) || amount <= 0) return undefined
  const unit = match[2]?.toLowerCase()
  if (unit === 'm') return Math.round(amount * 1_000_000)
  if (unit === 'k') return Math.round(amount * 1_000)
  return Math.round(amount)
}

function maskedKey(p: AiProviderInfo): string {
  return p.has_key ? '••••••••' : t('aiService.apiKeyNotSet')
}

function capabilityOptionsFor(caps: string[]) {
  const known = new Set(CAPABILITY_OPTIONS.map(c => c.id))
  const extras = caps
    .filter(cap => cap && !known.has(cap))
    .map(cap => ({ id: cap, labelKey: capabilityLabelKey(cap) ?? cap }))
  return [...CAPABILITY_OPTIONS, ...extras]
}

function capabilityLabel(option: { id: string; labelKey: string }) {
  if (option.labelKey === option.id) return option.id
  return t(option.labelKey)
}

function capabilityLabelById(cap: string) {
  const labelKey = capabilityLabelKey(cap)
  return labelKey ? t(labelKey) : cap
}

function capabilityLabelKey(cap: string) {
  return CAPABILITY_OPTIONS.find(c => c.id === cap)?.labelKey ?? CAPABILITY_LABEL_ALIASES[cap]
}

function fetchModelSearchText(model: AiModel) {
  return `${model.display_name ?? ''} ${model.id ?? ''} ${(model.capabilities ?? []).join(' ')}`.toLowerCase()
}

function normalizedFetchCapabilities(model: AiModel) {
  const caps = new Set((model.capabilities ?? []).filter(Boolean))
  if (caps.has('function_calling') || caps.has('tools') || caps.has('tool_use')) caps.add('tool_calling')
  if (caps.has('image') || caps.has('multimodal')) caps.add('vision')
  if (caps.has('embed') || caps.has('embeddings')) caps.add('embedding')

  const text = fetchModelSearchText(model)
  if (/\b(embed|embedding|embeddings)\b/.test(text) || /text-embedding|bge-|gte-|e5-|voyage-/.test(text)) {
    caps.add('embedding')
  }
  if (/vision|multimodal|image|qwen-vl|llava|pixtral|janus|gpt-4o|gemini|kimi-k2/.test(text)) {
    caps.add('vision')
  }
  if (/tool|function/.test(text)) {
    caps.add('tool_calling')
  }
  if (/\b(reason|reasoning|thinking)\b/.test(text) || /(^|[/_\-\s])(r1|o1|o3|o4|qwq)(?=$|[/_\-\s])/.test(text)) {
    caps.add('reasoning')
  }

  return caps
}

function hasFetchCapability(model: AiModel, cap: FetchGroupId): boolean {
  const caps = normalizedFetchCapabilities(model)
  if (cap === 'other') {
    return !FETCH_GROUP_ORDER.some(group => group !== 'other' && hasFetchCapability(model, group))
  }
  return caps.has(cap)
}

function primaryFetchCapability(model: AiModel): FetchGroupId {
  for (const cap of FETCH_GROUP_ORDER) {
    if (cap !== 'other' && hasFetchCapability(model, cap)) return cap
  }
  return 'other'
}

function fetchModelCapabilityBadges(model: AiModel) {
  return FETCH_GROUP_ORDER.filter(cap => cap !== 'other' && hasFetchCapability(model, cap))
}

function fetchGroupLabel(cap: FetchGroupId) {
  return cap === 'other' ? '其他' : capabilityLabelById(cap)
}

function toggleCapability(form: ModelForm, cap: string) {
  form.capabilities = form.capabilities.includes(cap)
    ? form.capabilities.filter(c => c !== cap)
    : [...form.capabilities, cap]
}

</script>

<template>
  <div class="ai-settings">
    <!-- Left: providers -->
    <div class="provider-list">
      <section class="source-section api-source-section">
        <div class="list-header">
          <span class="list-title">{{ t('aiService.title') }}</span>
          <button class="add-btn" @click="startAdd" :title="t('aiService.addProvider')">
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
          </button>
        </div>

        <div class="section-scroll">
          <div v-if="ai.settings.providers.length === 0 && !isAdding" class="no-providers">
            {{ t('aiService.noProviders') }}
          </div>

          <button
            v-for="p in ai.settings.providers"
            :key="p.id"
            class="provider-item"
            :class="{ active: selectedId === p.id }"
            @click="selectProvider(p.id)"
            @contextmenu.prevent="openProviderCtx($event, p)"
          >
            <span class="provider-initial">
              <img v-if="providerLogoUrl(p.name, p.base_url)" :src="providerLogoUrl(p.name, p.base_url)!" class="provider-logo" alt="" />
              <template v-else>{{ p.name[0]?.toUpperCase() ?? 'P' }}</template>
            </span>
            <span class="provider-name">{{ p.name }}</span>
            <span
              class="provider-switch"
              :class="{ on: p.enabled }"
              role="switch"
              :aria-checked="p.enabled"
              @click.stop="toggleProviderEnabled(p)"
            >
              <span class="switch-knob" />
            </span>
          </button>

          <button v-if="isAdding" class="provider-item active adding">
            <span class="provider-initial">+</span>
            <span class="provider-name">{{ t('aiService.addProvider') }}</span>
          </button>
        </div>
      </section>

    </div>

    <!-- Right: detail panel -->
    <div class="provider-detail">
      <!-- Add provider form -->
      <template v-if="isAdding">
        <div class="detail-header">
          <span class="detail-title">{{ t('aiService.addProvider') }}</span>
        </div>

        <div class="presets-row">
          <span class="field-label">{{ t('aiService.presets') }}</span>
          <div class="preset-btns">
            <button v-for="p in PRESETS" :key="p.label" class="preset-btn" @click="applyPreset(p)">
              {{ p.label }}
            </button>
          </div>
        </div>

        <div class="field-group">
          <div class="field-label">{{ t('aiService.providerName') }}</div>
          <input v-model="addForm.name" class="text-input" :placeholder="t('aiService.namePlaceholder')" />
        </div>
        <div class="field-group">
          <div class="field-label">{{ t('aiService.baseUrl') }}</div>
          <input v-model="addForm.base_url" class="text-input" placeholder="https://api.openai.com/v1" />
        </div>
        <div class="field-group">
          <div class="field-label">{{ t('aiService.serviceType') }}</div>
          <select v-model="addForm.kind" class="select-input">
            <option value="openai_compatible">{{ t('aiService.openaiCompat') }}</option>
            <option value="openrouter">{{ t('aiService.openrouter') }}</option>
            <option value="kimi">{{ t('aiService.kimi') }}</option>
            <option value="anthropic">{{ t('aiService.anthropic') }}</option>
          </select>
        </div>
        <div class="field-group">
          <div class="field-label">{{ t('aiService.apiKey') }}</div>
          <input v-model="addForm.api_key" type="password" class="text-input" :placeholder="t('aiService.apiKeyPlaceholder')" />
          <div class="field-note">{{ t('aiService.apiKeyNote') }}</div>
        </div>
        <div class="action-row">
          <button class="btn-primary" @click="submitAdd" :disabled="!addForm.name || !addForm.base_url">
            {{ t('aiService.add') }}
          </button>
          <button class="btn-ghost" @click="isAdding = false">{{ t('aiService.cancel') }}</button>
        </div>
      </template>

      <!-- No selection hint -->
      <div v-else-if="!selectedProvider" class="empty-hint">
        <p>{{ t('aiService.noProvidersHint') }}</p>
        <button class="btn-primary" @click="startAdd">{{ t('aiService.addProvider') }}</button>
      </div>

      <!-- Provider detail -->
      <template v-else>
        <div class="detail-header">
          <img
            v-if="providerLogoUrl(selectedProvider.name, selectedProvider.base_url)"
            :src="providerLogoUrl(selectedProvider.name, selectedProvider.base_url)!"
            class="detail-logo"
            alt=""
          />
          <span class="detail-title">{{ selectedProvider.name }}</span>
          <div class="header-actions">
            <button class="btn-ghost sm" @click="testConnection" :disabled="testStatus === 'testing'">
              {{ testStatus === 'testing' ? '…' : t('aiService.testConn') }}
            </button>
            <button
              class="toggle-switch detail-switch"
              :class="{ on: editEnabled }"
              role="switch"
              :aria-checked="editEnabled"
              :title="editEnabled ? t('aiService.enableOn') : t('aiService.enableOff')"
              @click="toggleEnabled"
            >
              <span class="switch-knob" />
            </button>
          </div>
        </div>

        <!-- Test result -->
        <div v-if="testMsg" class="test-result" :class="testStatus">{{ testMsg }}</div>

        <div class="field-group">
          <div class="field-label">{{ t('aiService.providerName') }}</div>
          <input v-model="editName" class="text-input" @blur="saveProvider" />
        </div>

        <div class="field-group">
          <div class="field-label">{{ t('aiService.apiKey') }}</div>
          <div class="key-row">
            <span class="key-display" :class="{ configured: selectedProvider.has_key }">
              {{ maskedKey(selectedProvider) }}
            </span>
            <button class="btn-ghost sm" @click="editKeyMode = !editKeyMode">
              {{ t('aiService.modifyKey') }}
            </button>
          </div>
          <template v-if="editKeyMode">
            <input
              v-model="editKey"
              type="password"
              class="text-input"
              style="margin-top:6px"
              :placeholder="t('aiService.apiKeyPlaceholder')"
              @keydown.enter="saveProvider"
              @blur="saveProvider"
            />
            <div class="field-note">{{ t('aiService.apiKeyNote') }}</div>
          </template>
        </div>

        <div class="field-group">
          <div class="field-label">{{ t('aiService.baseUrl') }}</div>
          <input v-model="editUrl" class="text-input" @blur="saveProvider" />
        </div>

        <div class="field-group">
          <div class="field-label">{{ t('aiService.serviceType') }}</div>
          <select v-model="editKind" class="select-input" @change="saveProvider">
            <option value="openai_compatible">{{ t('aiService.openaiCompat') }}</option>
            <option value="openrouter">{{ t('aiService.openrouter') }}</option>
            <option value="kimi">{{ t('aiService.kimi') }}</option>
            <option value="anthropic">{{ t('aiService.anthropic') }}</option>
          </select>
        </div>

        <!-- Fetch model selection dialog -->
        <Transition name="fetch-dialog">
          <div v-if="showFetchDialog" class="fetch-dialog-overlay" @click.self="showFetchDialog = false">
            <div class="fetch-dialog">
              <div class="fetch-dialog-header">
                <div>
                  <div class="fetch-dialog-title">{{ t('aiService.fetchDialogTitle') }}</div>
                  <div class="fetch-dialog-sub">{{ t('aiService.fetchDialogSub') }}</div>
                </div>
                <button class="fetch-dialog-close" @click="showFetchDialog = false">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                    <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
                  </svg>
                </button>
              </div>
              <div class="fetch-dialog-filter">
                <div class="fetch-search">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <circle cx="11" cy="11" r="7"/><line x1="16.5" y1="16.5" x2="21" y2="21"/>
                  </svg>
                  <input v-model="fetchSearch" placeholder="搜索模型名称或 ID..." />
                </div>
                <div class="fetch-cap-tabs">
                  <button
                    class="fetch-cap-tab"
                    :class="{ active: fetchCapability === 'all' }"
                    @click="fetchCapability = 'all'"
                  >
                    全部
                  </button>
                  <button
                    v-for="cap in FETCH_GROUP_ORDER"
                    :key="cap"
                    class="fetch-cap-tab"
                    :class="{ active: fetchCapability === cap }"
                    @click="fetchCapability = cap"
                  >
                    {{ fetchGroupLabel(cap) }}
                  </button>
                </div>
              </div>
              <div class="fetch-dialog-actions">
                <button class="btn-ghost xs" @click="selectAllFetched">{{ t('aiService.selectAll') }}</button>
                <button class="btn-ghost xs" @click="deselectAllFetched">{{ t('aiService.deselectAll') }}</button>
                <span class="fetch-selected-count">
                  已选 {{ fetchSelected.size }}，显示 {{ fetchFilteredModels.length }}/{{ fetchedModels.length }}
                </span>
              </div>
              <div class="fetch-dialog-list">
                <template v-if="fetchGroupedModels.length">
                  <div v-for="group in fetchGroupedModels" :key="group.id" class="fetch-model-group">
                    <div class="fetch-group-title">{{ group.label }}</div>
                    <label
                      v-for="m in group.models"
                      :key="m.id"
                      class="fetch-model-row"
                      :class="{ 'already-added': editModels.some(e => e.id === m.id) }"
                    >
                      <input
                        type="checkbox"
                        class="fetch-checkbox"
                        :checked="fetchSelected.has(m.id)"
                        :disabled="editModels.some(e => e.id === m.id)"
                        @change="toggleFetchSelect(m.id)"
                      />
                      <div class="fetch-model-info">
                        <span class="fetch-model-name">{{ m.display_name }}</span>
                        <span class="fetch-model-id">{{ m.id }}</span>
                      </div>
                      <div class="fetch-model-caps">
                        <span
                          v-for="cap in fetchModelCapabilityBadges(m)"
                          :key="cap"
                          class="fetch-cap-badge"
                        >
                          {{ fetchGroupLabel(cap) }}
                        </span>
                      </div>
                      <span v-if="editModels.some(e => e.id === m.id)" class="fetch-already-badge">{{ t('aiService.alreadyAdded') }}</span>
                    </label>
                  </div>
                </template>
                <div v-else class="fetch-empty">没有匹配的模型</div>
              </div>
              <div class="fetch-dialog-footer">
                <span v-if="fetchAvailableVisibleCount > 0" class="fetch-footer-hint">
                  当前筛选可添加 {{ fetchAvailableVisibleCount }} 个
                </span>
                <button class="btn-ghost sm" @click="showFetchDialog = false">{{ t('aiService.cancel') }}</button>
                <button class="btn-primary sm" @click="confirmFetchedSelection" :disabled="fetchSelected.size === 0">
                  {{ t('aiService.confirmAdd') }}
                </button>
              </div>
            </div>
          </div>
        </Transition>

        <!-- Models section -->
        <div class="models-section">
          <div class="models-header">
            <span class="field-label">{{ t('aiService.models') }}
              <span v-if="editModels.length" class="model-count">({{ editModels.length }})</span>
            </span>
            <div class="model-actions">
              <button class="btn-ghost sm" @click="fetchModels" :disabled="fetchStatus === 'fetching'">
                {{ fetchStatus === 'fetching' ? t('aiService.fetchingModels') : t('aiService.fetchModels') }}
              </button>
              <button class="btn-ghost sm" @click="showAddModel = !showAddModel">{{ t('aiService.addManual') }}</button>
            </div>
          </div>

          <div v-if="fetchErr" class="fetch-err">{{ fetchErr }}</div>

          <!-- Add model form -->
          <div v-if="showAddModel" class="add-model-form">
            <div class="model-form-grid">
              <label class="model-form-field model-form-id">
                <span>{{ t('aiService.modelId') }}</span>
                <input v-model="newModel.id" class="text-input sm" placeholder="gpt-4.1-mini" />
              </label>
              <label class="model-form-field">
                <span>{{ t('aiService.modelName') }}</span>
                <input v-model="newModel.display_name" class="text-input sm" :placeholder="t('aiService.modelName')" />
              </label>
              <label class="model-form-field model-form-context">
                <span>{{ t('aiService.contextLength') }}</span>
                <input v-model="newModel.context_length" class="text-input sm" placeholder="128K" />
              </label>
            </div>
            <div class="model-form-price-row">
              <label class="model-form-field" :class="{ 'full-width': newModel.capabilities.includes('embedding') }">
                <span>{{ newModel.capabilities.includes('embedding') ? '向量化价格' : '输入价格' }} <small>（元/百万tokens）</small></span>
                <input v-model="newModel.input_price" class="text-input sm" placeholder="如 2.0" type="number" min="0" step="0.01" />
              </label>
              <label v-if="!newModel.capabilities.includes('embedding')" class="model-form-field">
                <span>输出价格 <small>（元/百万tokens）</small></span>
                <input v-model="newModel.output_price" class="text-input sm" placeholder="如 8.0" type="number" min="0" step="0.01" />
              </label>
              <p v-else class="embedding-price-note">Embedding 模型仅计算输入 token，无输出费用</p>
            </div>
            <div class="capability-editor">
              <span class="capability-label">{{ t('aiService.capabilities') }}</span>
              <div class="capability-options">
                <button
                  v-for="cap in capabilityOptionsFor(newModel.capabilities)"
                  :key="cap.id"
                  class="cap-option"
                  :class="{ active: newModel.capabilities.includes(cap.id) }"
                  @click="toggleCapability(newModel, cap.id)"
                >
                  {{ capabilityLabel(cap) }}
                </button>
              </div>
            </div>
            <div class="model-form-actions">
              <button class="btn-primary sm" @click="submitAddModel">{{ t('aiService.add') }}</button>
              <button class="btn-ghost sm" @click="showAddModel = false">{{ t('aiService.cancel') }}</button>
            </div>
          </div>

          <div v-if="editModels.length === 0" class="no-models">{{ t('aiService.noModels') }}</div>

          <!-- Model list -->
          <div class="model-list">
            <div v-for="(m, idx) in editModels" :key="m.id" class="model-item">
              <template v-if="editModelIdx === idx">
                <!-- Inline edit -->
                <div class="model-edit-form">
                  <div class="model-edit-header">
                    <div class="model-main">
                      <div class="model-name-line">
                        <span class="model-display">{{ editModelForm.display_name || editModelForm.id }}</span>
                        <span v-for="cap in editModelForm.capabilities" :key="cap" class="cap-badge">{{ capabilityLabelById(cap) }}</span>
                        <span v-if="editModelForm.context_length" class="ctx-badge">{{ editModelForm.context_length }}</span>
                      </div>
                    </div>
                    <div class="model-btns">
                      <button class="icon-btn success" @click="saveEditModel" :title="t('aiService.save')">
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="20 6 9 17 4 12"/></svg>
                      </button>
                      <button class="icon-btn" @click="editModelIdx = null" :title="t('aiService.cancel')">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                      </button>
                    </div>
                  </div>
                  <div class="model-edit-body">
                    <div class="model-form-grid">
                      <label class="model-form-field model-form-id">
                        <span>{{ t('aiService.modelId') }}</span>
                        <input v-model="editModelForm.id" class="text-input sm" :placeholder="t('aiService.modelId')" />
                      </label>
                      <label class="model-form-field">
                        <span>{{ t('aiService.modelName') }}</span>
                        <input v-model="editModelForm.display_name" class="text-input sm" :placeholder="t('aiService.modelName')" />
                      </label>
                      <label class="model-form-field model-form-context">
                        <span>{{ t('aiService.contextLength') }}</span>
                        <input v-model="editModelForm.context_length" class="text-input sm" placeholder="128K" />
                      </label>
                    </div>
                    <div class="model-form-price-row">
                      <label class="model-form-field" :class="{ 'full-width': editModelForm.capabilities.includes('embedding') }">
                        <span>{{ editModelForm.capabilities.includes('embedding') ? '向量化价格' : '输入价格' }} <small>（元/百万tokens）</small></span>
                        <input v-model="editModelForm.input_price" class="text-input sm" placeholder="如 2.0" type="number" min="0" step="0.01" />
                      </label>
                      <label v-if="!editModelForm.capabilities.includes('embedding')" class="model-form-field">
                        <span>输出价格 <small>（元/百万tokens）</small></span>
                        <input v-model="editModelForm.output_price" class="text-input sm" placeholder="如 8.0" type="number" min="0" step="0.01" />
                      </label>
                      <p v-else class="embedding-price-note">Embedding 模型仅计算输入 token，无输出费用</p>
                    </div>
                    <div class="capability-editor">
                      <span class="capability-label">{{ t('aiService.capabilities') }}</span>
                      <div class="capability-options">
                        <button
                          v-for="cap in capabilityOptionsFor(editModelForm.capabilities)"
                          :key="cap.id"
                          class="cap-option"
                          :class="{ active: editModelForm.capabilities.includes(cap.id) }"
                          @click="toggleCapability(editModelForm, cap.id)"
                        >
                          {{ capabilityLabel(cap) }}
                        </button>
                      </div>
                    </div>

                    <!-- OpenRouter provider selection -->
                    <div v-if="isOpenRouterProvider" class="or-provider-section">
                      <div class="or-provider-header">
                        <span class="capability-label">Provider 偏好</span>
                        <button
                          class="btn-ghost xs"
                          :disabled="orEndpointStatus === 'fetching'"
                          @click="fetchOrEndpoints(editModelForm.id)"
                        >{{ orEndpointStatus === 'fetching' ? '获取中…' : '从 OpenRouter 获取' }}</button>
                      </div>
                      <!-- Fetched endpoint list -->
                      <template v-if="orEndpointStatus === 'ok' && orEndpoints.length > 0">
                        <div class="or-provider-hint">勾选并排序：排在前面的优先使用（不勾则 OpenRouter 自动选择）</div>
                        <div class="or-endpoint-list">
                          <label
                            v-for="ep in orEndpoints"
                            :key="ep.name"
                            class="or-endpoint-item"
                            :class="{ active: editModelForm.provider_order.includes(ep.name) }"
                          >
                            <input
                              type="checkbox"
                              :checked="editModelForm.provider_order.includes(ep.name)"
                              @change="toggleOrProvider(ep.name, editModelForm)"
                            />
                            <span class="or-ep-name">{{ ep.name }}</span>
                            <span v-if="ep.quantization" class="or-ep-quant">{{ ep.quantization.toUpperCase() }}</span>
                            <span v-if="editModelForm.provider_order.includes(ep.name)" class="or-ep-rank">
                              #{{ editModelForm.provider_order.indexOf(ep.name) + 1 }}
                            </span>
                          </label>
                        </div>
                      </template>
                      <!-- Fetch failed / not tried: text input fallback -->
                      <template v-else>
                        <div v-if="orEndpointErr" class="or-endpoint-err">{{ orEndpointErr }}</div>
                        <div class="or-provider-manual">
                          <input
                            class="text-input sm"
                            placeholder="Anthropic, Together, … （逗号分隔，按优先级排列）"
                            :value="editModelForm.provider_order.join(', ')"
                            @change="(e) => editModelForm.provider_order = (e.target as HTMLInputElement).value.split(',').map(s => s.trim()).filter(Boolean)"
                          />
                        </div>
                      </template>
                    </div>
                  </div>
                </div>
              </template>
              <template v-else>
                <div class="model-row">
                  <div class="model-main">
                    <div class="model-name-line">
                      <span class="model-display">{{ m.display_name }}</span>
                      <span v-for="cap in m.capabilities" :key="cap" class="cap-badge">{{ capabilityLabelById(cap) }}</span>
                      <span v-if="m.context_length" class="ctx-badge">{{ formatCtx(m.context_length) }}</span>
                      <span
                        v-if="defaultSel?.providerId === selectedId && defaultSel?.modelId === m.id"
                        class="default-badge"
                      >{{ t('aiService.isDefault') }}</span>
                    </div>
                    <div class="model-id-row">
                      <span class="model-id">{{ m.id }}</span>
                      <span
                        v-if="m.input_price_per_million != null || m.output_price_per_million != null || m.input_price_usd_per_million != null || m.output_price_usd_per_million != null"
                        class="price-hint"
                      >
                        <span v-if="priceCnyPerMillion(m.input_price_usd_per_million, m.input_price_per_million) != null">
                          入 {{ fmtPricePerMillion(priceCnyPerMillion(m.input_price_usd_per_million, m.input_price_per_million)) }}
                        </span>
                        <span v-if="priceCnyPerMillion(m.output_price_usd_per_million, m.output_price_per_million) != null">
                          出 {{ fmtPricePerMillion(priceCnyPerMillion(m.output_price_usd_per_million, m.output_price_per_million)) }}
                        </span>
                      </span>
                      <span v-if="m.provider_order && m.provider_order.length > 0" class="provider-order-hint" :title="m.provider_order.join(' → ')">
                        Provider: {{ m.provider_order.slice(0, 2).join(' → ') }}{{ m.provider_order.length > 2 ? ' …' : '' }}
                      </span>
                    </div>
                  </div>
                  <div class="model-btns">
                    <button
                      class="icon-btn"
                      :class="{ 'icon-btn-star': defaultSel?.providerId === selectedId && defaultSel?.modelId === m.id }"
                      @click="setDefaultModel(selectedId!, m.id)"
                      :title="t('aiService.setDefault')"
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.5"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>
                    </button>
                    <button class="icon-btn" @click="startEditModel(idx)" :title="t('aiService.editModel')">
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                    </button>
                    <button class="icon-btn danger" @click="removeModel(idx); saveProvider()" :title="t('aiService.deleteModel')">
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/></svg>
                    </button>
                  </div>
                </div>
              </template>
            </div>
          </div>

          <!-- Save indicator -->
          <div v-if="saveStatus" class="save-indicator" :class="saveStatus">
            {{ saveStatus === 'saving' ? '…' : t('settings.saved') }}
          </div>
        </div>
      </template>

    </div>

    <Teleport to="body">
      <div
        v-if="providerCtxMenu"
        class="provider-ctx-menu"
        :style="{ left: providerCtxMenu.x + 'px', top: providerCtxMenu.y + 'px' }"
        @click.stop
      >
        <button class="provider-ctx-item danger" @click="deleteProviderById(providerCtxMenu.provider)">
          {{ t('aiService.delete') }}
        </button>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
/* ── AI settings (existing) ─────────────────────────────────────────────── */
.ai-settings {
  display: flex;
  height: 100%;
  overflow: hidden;
  background: var(--bg-primary);
  color: var(--text-primary);
}

/* Left panel */
.provider-list {
  width: 240px;
  flex-shrink: 0;
  border-right: 1px solid var(--border-subtle);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 0;
  background: color-mix(in srgb, var(--bg-secondary) 42%, var(--bg-primary));
}

.source-section {
  min-height: 0;
  display: flex;
  flex-direction: column;
  padding: 14px 10px 10px;
}

.api-source-section {
  flex: 1 1 auto;
}

.section-scroll {
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  overflow-y: auto;
  padding-right: 2px;
}

.list-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 4px 10px;
}
.list-title {
  font-size: 13px;
  font-weight: 600;
  line-height: 1.35;
  color: var(--text-secondary);
  letter-spacing: 0;
}

.add-btn {
  color: var(--text-tertiary);
  width: 24px;
  height: 24px;
  border-radius: var(--radius-sm);
  display: flex;
  align-items: center;
  justify-content: center;
}
.add-btn:hover { background: var(--bg-hover); color: var(--accent); }
.add-btn:disabled { opacity: 0.45; cursor: default; }

.no-providers {
  font-size: 12px;
  color: var(--text-tertiary);
  padding: 9px 10px;
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-secondary) 58%, transparent);
  line-height: 1.45;
}

.provider-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  width: 100%;
  text-align: left;
  font-size: 13px;
  line-height: 1.35;
  font-weight: 500;
  color: var(--text-secondary);
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
  cursor: pointer;
}
.provider-item:hover { background: var(--bg-hover); }
.provider-item.active {
  background: var(--accent-light);
  color: var(--accent);
}

.provider-initial {
  width: 24px;
  height: 24px;
  border-radius: 7px;
  background: color-mix(in srgb, var(--bg-tertiary) 76%, var(--bg-primary));
  color: var(--text-secondary);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 11px;
  font-weight: 600;
  flex-shrink: 0;
}
.provider-item.active .provider-initial {
  background: color-mix(in srgb, var(--accent) 16%, var(--bg-primary));
  color: var(--accent);
}

.provider-logo {
  width: 16px;
  height: 16px;
  object-fit: contain;
  display: block;
}

.provider-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.detail-logo {
  width: 26px;
  height: 26px;
  object-fit: contain;
  flex-shrink: 0;
  border-radius: 6px;
}

.detail-initial {
  width: 28px;
  height: 28px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--accent) 14%, var(--bg-primary));
  color: var(--accent);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 13px;
  font-weight: 700;
  flex-shrink: 0;
}

.provider-switch,
.toggle-switch {
  flex-shrink: 0;
  position: relative;
  display: inline-flex;
  align-items: center;
  border-radius: var(--radius-pill);
  background: var(--bg-tertiary);
  cursor: pointer;
  transition: background 0.14s, box-shadow 0.14s;
  box-shadow: inset 0 0 0 1px var(--border-default);
}
.provider-switch {
  width: 30px;
  height: 17px;
}
.toggle-switch {
  width: 42px;
  height: 24px;
}
.provider-switch.on,
.toggle-switch.on {
  background: #2e7d32;
  box-shadow: inset 0 0 0 1px #2e7d32;
}
.provider-switch.disabled,
.toggle-switch.disabled {
  background: color-mix(in srgb, var(--bg-tertiary) 46%, var(--bg-primary));
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--border-default) 62%, transparent);
  cursor: not-allowed;
  opacity: 0.58;
}
.switch-knob {
  position: absolute;
  left: 2px;
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: var(--bg-primary);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.22);
  transition: transform 0.14s ease;
}
.toggle-switch .switch-knob {
  width: 20px;
  height: 20px;
}
.provider-switch.on .switch-knob {
  transform: translateX(13px);
}
.toggle-switch.on .switch-knob {
  transform: translateX(18px);
}

/* Right panel */
.provider-detail {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 24px 28px 28px;
  position: relative;
}

.empty-hint {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  min-height: 360px;
  gap: 14px;
  color: var(--text-secondary);
  font-size: 13px;
  line-height: 1.5;
}

.detail-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 22px;
}
.detail-title {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
  flex: 1;
}
.header-actions { display: flex; gap: 6px; align-items: center; }

.field-group { margin-bottom: 16px; }
.field-label {
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0;
  color: var(--text-secondary);
  margin-bottom: 6px;
  display: block;
}
.field-note {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  margin-top: 6px;
  line-height: 1.45;
}

.text-input {
  width: 100%;
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 92%, var(--bg-secondary));
  color: var(--text-primary);
  box-sizing: border-box;
  transition: border-color 0.12s, background 0.12s;
}
.text-input.sm { padding: 4px 7px; font-size: var(--font-size-xs); }
.text-input.mono { font-family: var(--font-mono); }
.text-input:focus {
  outline: none;
  border-color: var(--accent);
  background: var(--bg-primary);
}

.select-input {
  width: 100%;
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 92%, var(--bg-secondary));
  color: var(--text-primary);
}
.select-input:focus {
  outline: none;
  border-color: var(--accent);
  background: var(--bg-primary);
}

.key-row { display: flex; align-items: center; gap: 10px; }
.key-display {
  font-size: 12px;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
}
.key-display.configured { color: #2e7d32; }

/* Buttons */
.btn-primary {
  padding: 7px 14px;
  font-size: 13px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-md);
  transition: background 0.12s;
}
.btn-primary:hover:not(:disabled) { background: var(--accent-hover); }
.btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }
.btn-primary.sm { padding: 3px 10px; font-size: var(--font-size-xs); }

.btn-ghost {
  padding: 6px 10px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s, border-color 0.12s;
}
.btn-ghost:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
  border-color: color-mix(in srgb, var(--accent) 30%, var(--border-default));
}
.btn-ghost:disabled { opacity: 0.5; cursor: not-allowed; }
.btn-ghost.sm { padding: 3px 8px; font-size: var(--font-size-xs); }
.btn-ghost.xs { padding: 2px 6px; font-size: 11px; }
.btn-ghost.danger { color: #cc3333; }
.btn-ghost.danger:hover { background: color-mix(in srgb, #cc3333 10%, transparent); }

.detail-switch:hover {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, #2e7d32 40%, var(--border-default));
}

.action-row { display: flex; gap: 8px; margin-top: 8px; }

/* Test result */
.test-result {
  font-size: var(--font-size-xs);
  padding: 6px 10px;
  border-radius: var(--radius-sm);
  margin-bottom: 14px;
}
.test-result.ok { background: color-mix(in srgb, #2e7d32 12%, transparent); color: #2e7d32; }
.test-result.fail { background: color-mix(in srgb, #cc3333 12%, transparent); color: #cc3333; }

/* Presets */
.presets-row { margin-bottom: 16px; }
.preset-btns { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 6px; }
.preset-btn {
  padding: 4px 10px;
  font-size: 12px;
  font-weight: 500;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-pill);
  color: var(--text-secondary);
  background: var(--bg-secondary);
  transition: all 0.1s;
}
.preset-btn:hover { background: var(--bg-hover); color: var(--accent); border-color: var(--accent); }

/* Models section */
.models-section { margin-top: 8px; }
.models-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 10px;
}
.models-header .field-label {
  font-size: 13px;
  margin-bottom: 0;
}
.model-count { color: var(--text-tertiary); font-size: 12px; margin-left: 5px; font-weight: 400; }
.model-actions { display: flex; gap: 8px; align-items: center; }
.model-actions .btn-ghost.sm {
  height: 28px;
  padding: 0 12px;
  font-size: 12px;
  border-radius: var(--radius-pill);
}

.fetch-err {
  font-size: var(--font-size-xs);
  color: #cc3333;
  padding: 4px 8px;
  background: color-mix(in srgb, #cc3333 8%, transparent);
  border-radius: var(--radius-sm);
  margin-bottom: 8px;
}

.add-model-form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  align-items: stretch;
  padding: 13px 14px;
  background: color-mix(in srgb, var(--bg-secondary) 76%, var(--bg-primary));
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  margin-bottom: 10px;
}

.model-form-grid {
  display: grid;
  width: 100%;
  max-width: 100%;
  grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
  gap: 10px;
  align-items: end;
}
.model-form-price-row {
  display: grid;
  width: 100%;
  max-width: 100%;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  margin-top: 0;
  align-items: end;
}
.model-form-field.full-width { grid-column: 1 / -1; }
.embedding-price-note {
  margin: 0;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  align-self: center;
  padding-bottom: 2px;
}
.model-form-field small { font-weight: 400; opacity: 0.72; }

.model-form-field {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.model-form-field span,
.capability-label {
  font-size: 11.5px;
  line-height: 1.25;
  font-weight: 600;
  color: var(--text-tertiary);
}

.model-form-field .text-input.sm {
  height: 32px;
  min-width: 0;
  padding: 5px 10px;
  font-size: 13px;
}

.capability-editor {
  display: grid;
  grid-template-columns: 52px 1fr;
  gap: 10px;
  align-items: start;
}

.capability-options {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  min-width: 0;
}

.cap-option {
  height: 28px;
  padding: 0 12px;
  border-radius: var(--radius-pill);
  border: 1px solid var(--border-default);
  background: color-mix(in srgb, var(--bg-primary) 88%, var(--bg-secondary));
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 500;
  line-height: 1;
  transition: background 0.12s, color 0.12s, border-color 0.12s;
}

.cap-option:hover {
  border-color: color-mix(in srgb, var(--accent) 42%, var(--border-default));
  color: var(--text-primary);
}

.cap-option.active {
  border-color: color-mix(in srgb, var(--accent) 62%, var(--border-default));
  background: var(--accent-light);
  color: var(--accent);
}

.model-form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
}

.no-models {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  padding: 8px 2px;
  line-height: 1.45;
}

.model-list { display: flex; flex-direction: column; gap: 6px; }

.model-item {
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  overflow: hidden;
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
}

.model-row {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 58px;
  padding: 9px 12px;
  background: transparent;
}
.model-row:hover { background: var(--bg-hover); }
.model-row:hover .model-btns .icon-btn { opacity: 1; }

.model-main {
  flex: 1;
  min-width: 0;
}

.model-name-line {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  min-width: 0;
}

.model-display {
  font-size: 14px;
  font-weight: 500;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex-shrink: 0;
  max-width: 200px;
}
.model-id-row {
  display: flex; align-items: center; gap: 10px; margin-top: 3px;
}
.model-id {
  font-size: 12px;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.price-hint {
  display: flex; gap: 6px; flex-shrink: 0;
  font-size: 11px; color: var(--text-tertiary);
}
.price-hint span { white-space: nowrap; }

.provider-order-hint {
  font-size: 11px;
  color: var(--accent, #6366f1);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 200px;
}

.or-provider-section {
  margin-top: 10px;
  border-top: 1px solid var(--border-default, #e5e7eb);
  padding-top: 10px;
}
.or-provider-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
}
.btn-ghost.xs {
  font-size: 11px;
  padding: 2px 8px;
  height: auto;
}
.or-provider-hint {
  font-size: 11px;
  color: var(--text-tertiary);
  margin-bottom: 6px;
}
.or-endpoint-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 180px;
  overflow-y: auto;
}
.or-endpoint-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 12px;
  border: 1px solid transparent;
  transition: background 0.1s;
}
.or-endpoint-item:hover { background: var(--bg-hover, #f3f4f6); }
.or-endpoint-item.active { border-color: var(--accent, #6366f1); background: var(--accent-light, #eef2ff); }
.or-endpoint-item input[type="checkbox"] { flex-shrink: 0; accent-color: var(--accent, #6366f1); }
.or-ep-name { flex: 1; font-weight: 500; color: var(--text-primary); }
.or-ep-quant {
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 4px;
  background: var(--bg-secondary, #f9fafb);
  border: 1px solid var(--border-default, #e5e7eb);
  color: var(--text-secondary);
  font-family: monospace;
}
.or-ep-rank {
  font-size: 11px;
  font-weight: 600;
  color: var(--accent, #6366f1);
  min-width: 20px;
  text-align: right;
}
.or-endpoint-err {
  font-size: 11px;
  color: var(--error, #ef4444);
  margin-bottom: 6px;
}
.or-provider-manual input { width: 100%; }

.cap-badge {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 8px;
  background: color-mix(in srgb, #2e7d32 13%, transparent);
  color: #2e7d32;
  font-weight: 500;
  white-space: nowrap;
  flex-shrink: 0;
}
.ctx-badge {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 8px;
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  white-space: nowrap;
  flex-shrink: 0;
}
.default-badge {
  font-size: 11px;
  padding: 2px 7px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--accent) 13%, transparent);
  color: var(--accent);
  white-space: nowrap;
  flex-shrink: 0;
}

.model-btns {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
  align-items: center;
}

.icon-btn {
  width: 28px;
  height: 28px;
  border-radius: var(--radius-sm);
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  opacity: 0.55;
  transition: background 0.1s, color 0.1s, opacity 0.1s;
  flex-shrink: 0;
}
.icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); opacity: 1; }
.icon-btn.danger:hover { background: color-mix(in srgb, #cc3333 10%, transparent); color: #cc3333; opacity: 1; }
.icon-btn.success:hover { background: color-mix(in srgb, #2e7d32 10%, transparent); color: #2e7d32; opacity: 1; }
.icon-btn-star { color: #f59e0b; opacity: 1; }
.icon-btn-star:hover { background: color-mix(in srgb, #f59e0b 10%, transparent) !important; color: #f59e0b !important; }

.model-edit-form {
  display: flex;
  flex-direction: column;
  background: transparent;
}

.model-edit-header {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 54px;
  padding: 9px 12px;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-hover) 72%, transparent);
}
.model-edit-header .model-main {
  display: flex;
  align-items: center;
}
.model-edit-header .model-btns .icon-btn { opacity: 1; }

.model-edit-body {
  display: flex;
  flex-direction: column;
  min-width: 0;
  gap: 12px;
  padding: 12px 14px 14px;
}

.save-indicator {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  margin-top: 8px;
  text-align: right;
}
.save-indicator.saved { color: #2e7d32; }

/* Fetch model selection dialog */
.fetch-dialog-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2000;
  padding: 20px;
}

.fetch-dialog {
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  width: 100%;
  max-width: 680px;
  max-height: min(680px, 85vh);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.fetch-dialog-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  padding: 16px 18px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.fetch-dialog-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 2px;
}

.fetch-dialog-sub {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.fetch-dialog-close {
  color: var(--text-tertiary);
  padding: 2px;
  border-radius: var(--radius-sm);
  flex-shrink: 0;
  margin-top: 1px;
}
.fetch-dialog-close:hover { background: var(--bg-hover); color: var(--text-primary); }

.fetch-dialog-filter {
  padding: 10px 18px 8px;
  border-bottom: 1px solid var(--border-subtle);
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex-shrink: 0;
}

.fetch-search {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 10px;
  color: var(--text-tertiary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 92%, var(--bg-secondary));
}

.fetch-search input {
  flex: 1;
  min-width: 0;
  border: 0;
  outline: none;
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
}

.fetch-search input::placeholder {
  color: var(--text-tertiary);
}

.fetch-cap-tabs {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.fetch-cap-tab {
  height: 24px;
  padding: 0 9px;
  border-radius: var(--radius-pill);
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  transition: background 0.12s, color 0.12s, border-color 0.12s;
}

.fetch-cap-tab:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

.fetch-cap-tab.active {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-primary));
  border-color: color-mix(in srgb, var(--accent) 28%, var(--border-default));
}

.fetch-dialog-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 18px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  background: color-mix(in srgb, var(--bg-secondary) 50%, var(--bg-primary));
}

.fetch-selected-count {
  margin-left: auto;
  font-size: 11px;
  color: var(--text-tertiary);
}

.fetch-dialog-list {
  flex: 1;
  overflow-y: auto;
  padding: 4px 0 8px;
}

.fetch-model-group {
  padding-top: 4px;
}

.fetch-group-title {
  position: sticky;
  top: 0;
  z-index: 1;
  padding: 6px 18px 5px;
  font-size: 11px;
  font-weight: 700;
  color: var(--text-tertiary);
  background: color-mix(in srgb, var(--bg-primary) 94%, transparent);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
}

.fetch-model-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 18px;
  cursor: pointer;
  transition: background 0.1s;
}
.fetch-model-row:hover { background: var(--bg-hover); }
.fetch-model-row.already-added { opacity: 0.55; cursor: default; }
.fetch-model-row.already-added:hover { background: transparent; }

.fetch-checkbox {
  appearance: none;
  -webkit-appearance: none;
  width: 16px;
  height: 16px;
  flex-shrink: 0;
  border: 1.5px solid color-mix(in srgb, var(--text-tertiary) 36%, var(--border-default));
  border-radius: 5px;
  background: color-mix(in srgb, var(--bg-primary) 96%, var(--bg-secondary));
  cursor: pointer;
  display: inline-grid;
  place-content: center;
  transition: background 0.12s, border-color 0.12s, box-shadow 0.12s;
}

.fetch-checkbox:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--accent) 42%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 5%, var(--bg-primary));
}

.fetch-checkbox:checked {
  border-color: var(--accent);
  background: var(--accent);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 12%, transparent);
}

.fetch-checkbox:checked::after {
  content: '';
  width: 4px;
  height: 8px;
  margin-top: -1px;
  border: solid #fff;
  border-width: 0 2px 2px 0;
  transform: rotate(45deg);
}

.fetch-checkbox:disabled {
  opacity: 0.55;
  background: var(--bg-tertiary);
  border-color: var(--border-default);
}
.fetch-model-row.already-added .fetch-checkbox { cursor: not-allowed; }

.fetch-model-info { flex: 1; min-width: 0; }
.fetch-model-name {
  display: block;
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.fetch-model-id {
  display: block;
  font-size: 11px;
  color: var(--text-tertiary);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.fetch-model-caps {
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
  justify-content: flex-end;
  max-width: 210px;
  flex-shrink: 0;
}

.fetch-cap-badge {
  font-size: 10px;
  line-height: 1.2;
  padding: 2px 6px;
  border-radius: var(--radius-pill);
  color: color-mix(in srgb, var(--accent) 86%, #1f2937);
  background: color-mix(in srgb, var(--accent) 9%, var(--bg-primary));
  white-space: nowrap;
}

.fetch-already-badge {
  font-size: 10px;
  padding: 1px 7px;
  border-radius: 8px;
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.fetch-empty {
  padding: 28px 18px;
  text-align: center;
  font-size: 12px;
  color: var(--text-tertiary);
}

.fetch-dialog-footer {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.fetch-footer-hint {
  margin-right: auto;
  font-size: 11px;
  color: var(--text-tertiary);
}

.fetch-dialog-enter-active,
.fetch-dialog-leave-active {
  transition: opacity 0.15s ease;
}
.fetch-dialog-enter-active .fetch-dialog,
.fetch-dialog-leave-active .fetch-dialog {
  transition: transform 0.15s ease, opacity 0.15s ease;
}
.fetch-dialog-enter-from,
.fetch-dialog-leave-to {
  opacity: 0;
}
.fetch-dialog-enter-from .fetch-dialog,
.fetch-dialog-leave-to .fetch-dialog {
  transform: scale(0.96);
  opacity: 0;
}

:global(.provider-ctx-menu) {
  position: fixed;
  z-index: 2200;
  min-width: 150px;
  padding: 5px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  box-shadow: var(--shadow-md);
}

:global(.provider-ctx-item) {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 6px 10px;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  text-align: left;
  transition: background 0.08s, color 0.08s;
}

:global(.provider-ctx-item:hover) {
  background: var(--accent);
  color: #fff;
}

:global(.provider-ctx-item.danger) {
  color: #cc3333;
}

:global(.provider-ctx-item.danger:hover) {
  background: #cc3333;
  color: #fff;
}
</style>
