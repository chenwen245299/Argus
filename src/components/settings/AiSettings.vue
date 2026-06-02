<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useAiStore } from '../../stores/ai'
import { useCliStore } from '../../stores/cli'
import type { AiModel, AiProviderInfo, AiProviderInput, CliOutputPolish, CliTool, ModelSelection } from '../../types'

const { t } = useI18n()
const ai = useAiStore()
const cliStore = useCliStore()

// ── Presets ───────────────────────────────────────────────────────────────────

const PRESETS = [
  { label: 'OpenAI',       base_url: 'https://api.openai.com/v1',         kind: 'openai_compatible' },
  { label: 'OpenRouter',   base_url: 'https://openrouter.ai/api/v1',      kind: 'openrouter' },
  { label: 'DeepSeek',     base_url: 'https://api.deepseek.com/v1',       kind: 'openai_compatible' },
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
}

function emptyModelForm(): ModelForm {
  return { id: '', display_name: '', context_length: '', capabilities: [], input_price: '', output_price: '' }
}

// ── State ─────────────────────────────────────────────────────────────────────

const selectedId    = ref<string | null>(null)
const isAdding      = ref(false)
const activeSource  = ref<'provider' | 'cli' | 'polish'>('provider')

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

// Default model selection
const defaultSel    = ref<ModelSelection | null>(null)

// Save feedback
const saveStatus    = ref<'' | 'saving' | 'saved'>('')
const providerCtxMenu = ref<{ x: number; y: number; provider: AiProviderInfo } | null>(null)

// ── CLI tools ────────────────────────────────────────────────────────────────

const cliSelectedId  = ref<string | null>(null)
const cliDetecting   = ref(false)
const cliTestResult  = ref<Record<string, { ok: boolean; msg: string }>>({})
const cliTestingId   = ref<string | null>(null)
const cliShowAddForm = ref(false)
const cliSavingId    = ref<string | null>(null)

const newCliTool = ref<Omit<CliTool, 'id' | 'detected' | 'version'>>({
  name: '', command: '', args_template: ['-p', '{prompt}'], enabled: false,
})
const newCliArgsRaw = ref('-p {prompt}')
const editCliArgsRaw = ref('')

const selectedCliTool = computed(() =>
  cliStore.settings.tools.find(t => t.id === cliSelectedId.value) ?? null
)

// ── CLI output polish ─────────────────────────────────────────────────────────

const polishForm = ref<CliOutputPolish>({ ...cliStore.settings.polish })
const polishSaving = ref(false)

async function savePolish() {
  polishSaving.value = true
  try {
    await cliStore.savePolish({ ...polishForm.value })
  } finally {
    polishSaving.value = false
  }
}

function openPolish() {
  polishForm.value = { ...cliStore.settings.polish }
  activeSource.value = 'polish'
  cliSelectedId.value = null
  cliShowAddForm.value = false
  selectedId.value = null
  isAdding.value = false
}

onMounted(async () => {
  await Promise.all([ai.load(), cliStore.load()])
  defaultSel.value = ai.defaultSelection
  if (ai.settings.providers.length > 0) {
    selectProvider(ai.settings.providers[0].id)
  } else if (cliStore.settings.tools.length > 0) {
    selectCliTool(cliStore.settings.tools[0].id)
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
  activeSource.value = 'provider'
  isAdding.value = false
  cliShowAddForm.value = false
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
  activeSource.value = 'provider'
  isAdding.value = true
  cliShowAddForm.value = false
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
  else if (cliStore.settings.tools.length > 0) selectCliTool(cliStore.settings.tools[0].id)
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

// ── CLI tools ────────────────────────────────────────────────────────────────

function selectCliTool(id: string) {
  closeProviderCtx()
  activeSource.value = 'cli'
  isAdding.value = false
  cliSelectedId.value = id
  cliShowAddForm.value = false
  cliTestResult.value = {}
  const tool = cliStore.settings.tools.find(t => t.id === id)
  editCliArgsRaw.value = tool ? tool.args_template.join(' ') : ''
}

function startCliAdd() {
  closeProviderCtx()
  activeSource.value = 'cli'
  isAdding.value = false
  cliSelectedId.value = null
  cliShowAddForm.value = true
  cliTestResult.value = {}
  newCliTool.value = { name: '', command: '', args_template: ['-p', '{prompt}'], enabled: false }
  newCliArgsRaw.value = '-p {prompt}'
}

async function detectCliTools() {
  closeProviderCtx()
  activeSource.value = 'cli'
  isAdding.value = false
  cliDetecting.value = true
  try {
    await cliStore.detect()
  } finally {
    cliDetecting.value = false
  }
  if (cliStore.settings.tools.length > 0 && !cliSelectedId.value) {
    selectCliTool(cliStore.settings.tools[0].id)
  }
}

async function testCliTool(id: string) {
  cliTestingId.value = id
  cliTestResult.value[id] = { ok: false, msg: '' }
  try {
    const msg = await cliStore.testTool(id)
    cliTestResult.value[id] = { ok: true, msg }
  } catch (e: unknown) {
    cliTestResult.value[id] = { ok: false, msg: String(e) }
  } finally {
    cliTestingId.value = null
  }
}

async function toggleCliEnabled(tool: CliTool) {
  await cliStore.saveTool({ ...tool, enabled: !tool.enabled })
}

async function saveCliField(tool: CliTool, field: keyof CliTool, value: unknown) {
  cliSavingId.value = tool.id
  await cliStore.saveTool({ ...tool, [field]: value })
  cliSavingId.value = null
}

async function saveCliArgs(tool: CliTool) {
  const parsed = editCliArgsRaw.value
    .split(/\s+/)
    .filter(s => s.length > 0)
  await saveCliField(tool, 'args_template', parsed)
}

async function deleteCliTool(id: string) {
  if (!confirm(t('cliSettings.deleteConfirm'))) return
  await cliStore.deleteTool(id)
  cliSelectedId.value = null
  if (cliStore.settings.tools.length > 0) {
    selectCliTool(cliStore.settings.tools[0].id)
  }
}

async function addCliTool() {
  if (!newCliTool.value.name || !newCliTool.value.command) return
  const parsed = newCliArgsRaw.value.split(/\s+/).filter(s => s.length > 0)
  const id = crypto.randomUUID()
  await cliStore.saveTool({
    id,
    name: newCliTool.value.name,
    command: newCliTool.value.command,
    args_template: parsed,
    enabled: false,
    detected: false,
    version: undefined,
  })
  cliShowAddForm.value = false
  selectCliTool(id)
}

function applyCliPreset(preset: 'claude' | 'codex') {
  if (preset === 'claude') {
    newCliTool.value.name = 'Claude Code'
    newCliTool.value.command = 'claude'
    newCliArgsRaw.value = '-p {prompt}'
  } else {
    newCliTool.value.name = 'Codex CLI'
    newCliTool.value.command = 'codex'
    newCliArgsRaw.value = '-q {prompt}'
  }
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
  const { id, display_name, context_length, capabilities, input_price, output_price } = newModel.value
  if (!id) return
  editModels.value.push({
    id,
    display_name: display_name || id,
    context_length: parseContextLength(context_length),
    capabilities: [...capabilities],
    enabled: true,
    input_price_per_million: parsePrice(input_price),
    output_price_per_million: parsePrice(output_price),
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
  }
}

function submitEditModel() {
  if (editModelIdx.value === null) return
  const { id, display_name, context_length, capabilities, input_price, output_price } = editModelForm.value
  editModels.value[editModelIdx.value] = {
    ...editModels.value[editModelIdx.value],
    id,
    display_name: display_name || id,
    context_length: parseContextLength(context_length),
    capabilities: [...capabilities],
    enabled: true,
    input_price_per_million: parsePrice(input_price),
    output_price_per_million: parsePrice(output_price),
  }
  editModelIdx.value = null
}

async function saveEditModel() {
  submitEditModel()
  await saveProvider()
}

function removeModel(idx: number) { editModels.value.splice(idx, 1) }

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

function cliBrand(tool: CliTool): 'claude' | 'codex' | null {
  const haystack = `${tool.name} ${tool.command}`.toLowerCase()
  if (haystack.includes('claude') || haystack.includes('anthropic')) return 'claude'
  if (haystack.includes('codex') || haystack.includes('openai')) return 'codex'
  return null
}

function cliLogoUrl(tool: CliTool): string | null {
  const brand = cliBrand(tool)
  if (brand === 'claude') {
    return new URL('../../assets/providers/claude.svg', import.meta.url).href
  }
  if (brand === 'codex') {
    return new URL('../../assets/providers/openai.svg', import.meta.url).href
  }
  return null
}

function cliDisplayName(tool: CliTool): string {
  const brand = cliBrand(tool)
  if (brand === 'claude') return 'Claude Code'
  if (brand === 'codex') return 'Codex'
  return tool.name
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
  if (/vision|multimodal|image|qwen-vl|llava|pixtral|janus|gpt-4o|gemini/.test(text)) {
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

function inputValue(e: Event): string {
  return (e.target as HTMLInputElement).value
}
</script>

<template>
  <div class="ai-settings">
    <!-- Left: providers and CLI tools -->
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
            :class="{ active: activeSource === 'provider' && selectedId === p.id }"
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

      <section class="source-section cli-source-section">
        <div class="list-header">
          <span class="list-title">{{ t('aiService.cliToolsLabel') }}</span>
          <div class="list-actions">
            <button class="add-btn" @click="detectCliTools" :disabled="cliDetecting" :title="t('cliSettings.detect')">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
                <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/>
                <path d="M3 3v5h5"/>
                <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/>
                <path d="M16 16h5v5"/>
              </svg>
            </button>
            <button class="add-btn" @click="startCliAdd" :title="t('cliSettings.addTool')">
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
              </svg>
            </button>
          </div>
        </div>

        <div class="section-scroll">
          <div v-if="cliStore.settings.tools.length === 0 && !cliShowAddForm" class="no-providers">
            {{ t('cliSettings.noTools') }}
          </div>

          <button
            v-for="tool in cliStore.settings.tools"
            :key="tool.id"
            class="provider-item"
            :class="{ active: activeSource === 'cli' && cliSelectedId === tool.id }"
            @click="selectCliTool(tool.id)"
          >
            <span class="provider-initial">
              <img v-if="cliLogoUrl(tool)" :src="cliLogoUrl(tool)!" class="provider-logo" alt="" />
              <template v-else>{{ cliDisplayName(tool)[0]?.toUpperCase() ?? 'C' }}</template>
            </span>
            <span class="provider-name">{{ cliDisplayName(tool) }}</span>
            <span
              class="provider-switch"
              :class="{ on: tool.enabled && tool.detected, disabled: !tool.detected }"
              role="switch"
              :aria-checked="tool.enabled && tool.detected"
              :aria-disabled="!tool.detected"
              :title="!tool.detected ? t('cliSettings.notDetected') : (tool.enabled ? t('cliSettings.on') : t('cliSettings.off'))"
              @click.stop="tool.detected && toggleCliEnabled(tool)"
            >
              <span class="switch-knob" />
            </span>
          </button>

          <button v-if="cliShowAddForm" class="provider-item active adding">
            <span class="provider-initial">+</span>
            <span class="provider-name">{{ t('cliSettings.addTool') }}</span>
          </button>
        </div>

        <!-- Polish entry -->
        <button
          class="provider-item polish-item"
          :class="{ active: activeSource === 'polish' }"
          @click="openPolish"
        >
          <span class="provider-initial">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="m9.06 11.9 8.07-8.06a2.85 2.85 0 1 1 4.03 4.03l-8.06 8.08"/>
              <path d="M7.07 14.94c-1.66 0-3 1.35-3 3.02 0 1.33-2.5 1.52-2 2.02 1.08 1.1 2.49 2.02 4 2.02 2.2 0 4-1.8 4-4.04a3.01 3.01 0 0 0-3-3.02z"/>
            </svg>
          </span>
          <span class="provider-name">AI 润色</span>
          <span class="provider-switch" :class="{ on: cliStore.settings.polish.enabled }" style="pointer-events:none">
            <span class="switch-knob" />
          </span>
        </button>
      </section>
    </div>

    <!-- Right: detail panel -->
    <div class="provider-detail">
      <template v-if="activeSource === 'provider'">
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
            <option value="openrouter">OpenRouter（支持 PDF）</option>
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
            <option value="openrouter">OpenRouter（支持 PDF）</option>
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
                      <span v-if="m.input_price_per_million != null || m.output_price_per_million != null" class="price-hint">
                        <span v-if="m.input_price_per_million != null">入 ¥{{ m.input_price_per_million }}/M</span>
                        <span v-if="m.output_price_per_million != null">出 ¥{{ m.output_price_per_million }}/M</span>
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
      </template>

      <template v-else-if="activeSource === 'cli'">
        <template v-if="cliShowAddForm">
          <div class="detail-header">
            <span class="detail-title">{{ t('cliSettings.addTool') }}</span>
          </div>

          <div class="presets-row">
            <span class="field-label">{{ t('cliSettings.presets') }}</span>
            <div class="preset-btns">
              <button class="preset-btn" @click="applyCliPreset('claude')">Claude Code</button>
              <button class="preset-btn" @click="applyCliPreset('codex')">Codex CLI</button>
            </div>
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.toolName') }}</div>
            <input v-model="newCliTool.name" class="text-input" :placeholder="t('cliSettings.toolNamePh')" />
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.command') }}</div>
            <input v-model="newCliTool.command" class="text-input mono" placeholder="claude" />
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.argsTemplate') }}</div>
            <input v-model="newCliArgsRaw" class="text-input mono" placeholder="-p {prompt}" />
            <div class="field-note">{{ t('cliSettings.argsHint') }}</div>
          </div>

          <div class="action-row">
            <button class="btn-primary" @click="addCliTool" :disabled="!newCliTool.name || !newCliTool.command">
              {{ t('cliSettings.add') }}
            </button>
            <button class="btn-ghost" @click="cliShowAddForm = false">{{ t('cliSettings.cancel') }}</button>
          </div>
        </template>

        <div v-else-if="!selectedCliTool" class="empty-hint">
          <p>{{ t('cliSettings.selectOrAdd') }}</p>
          <button class="btn-primary" @click="startCliAdd">{{ t('cliSettings.addTool') }}</button>
        </div>

        <template v-else>
          <div class="detail-header">
            <img v-if="cliLogoUrl(selectedCliTool)" :src="cliLogoUrl(selectedCliTool)!" class="detail-logo" alt="" />
            <span v-else class="detail-initial">{{ cliDisplayName(selectedCliTool)[0]?.toUpperCase() ?? 'C' }}</span>
            <span class="detail-title">{{ cliDisplayName(selectedCliTool) }}</span>
            <div class="header-actions">
              <button
                class="btn-ghost sm"
                :disabled="cliTestingId === selectedCliTool.id"
                @click="testCliTool(selectedCliTool.id)"
              >
                {{ cliTestingId === selectedCliTool.id ? t('cliSettings.testing') : t('cliSettings.test') }}
              </button>
              <button
                class="toggle-switch detail-switch"
                :class="{ on: selectedCliTool.enabled && selectedCliTool.detected, disabled: !selectedCliTool.detected }"
                role="switch"
                :aria-checked="selectedCliTool.enabled && selectedCliTool.detected"
                :aria-disabled="!selectedCliTool.detected"
                :title="!selectedCliTool.detected ? t('cliSettings.notDetected') : (selectedCliTool.enabled ? t('cliSettings.on') : t('cliSettings.off'))"
                :disabled="!selectedCliTool.detected"
                @click="toggleCliEnabled(selectedCliTool)"
              >
                <span class="switch-knob" />
              </button>
            </div>
          </div>

          <div class="status-row">
            <span class="mini-badge" :class="selectedCliTool.detected ? 'badge-ok' : 'badge-warn'">
              {{ selectedCliTool.detected ? t('cliSettings.detected') : t('cliSettings.notDetected') }}
              <span v-if="selectedCliTool.version" class="version-text"> · {{ selectedCliTool.version }}</span>
            </span>
          </div>

          <div
            v-if="cliTestResult[selectedCliTool.id]?.msg"
            class="test-result"
            :class="cliTestResult[selectedCliTool.id].ok ? 'ok' : 'fail'"
          >
            {{ cliTestResult[selectedCliTool.id].msg }}
          </div>

          <div v-if="!selectedCliTool.detected" class="not-detected-hint">
            {{ t('cliSettings.notDetectedHint', { cmd: selectedCliTool.command }) }}
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.toolName') }}</div>
            <input
              :value="selectedCliTool.name"
              class="text-input"
              @blur="saveCliField(selectedCliTool, 'name', inputValue($event))"
            />
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.command') }}</div>
            <input
              :value="selectedCliTool.command"
              class="text-input mono"
              @blur="saveCliField(selectedCliTool, 'command', inputValue($event))"
            />
          </div>

          <div class="field-group">
            <div class="field-label">{{ t('cliSettings.argsTemplate') }}</div>
            <input
              v-model="editCliArgsRaw"
              class="text-input mono"
              placeholder="-p {prompt}"
              @blur="saveCliArgs(selectedCliTool)"
            />
            <div class="field-note">{{ t('cliSettings.argsHint') }}</div>
          </div>

          <div class="danger-zone">
            <button class="btn-ghost danger" @click="deleteCliTool(selectedCliTool.id)">
              {{ t('cliSettings.delete') }}
            </button>
            <span v-if="cliSavingId === selectedCliTool.id" class="saving-text">…</span>
          </div>
        </template>
      </template>

      <!-- Polish config panel -->
      <template v-if="activeSource === 'polish'">
        <div class="detail-header">
          <span class="detail-initial">✦</span>
          <span class="detail-title">CLI 输出 AI 润色</span>
          <div class="header-actions">
            <button
              class="toggle-switch detail-switch"
              :class="{ on: polishForm.enabled }"
              role="switch"
              :aria-checked="polishForm.enabled"
              @click="polishForm.enabled = !polishForm.enabled"
            >
              <span class="switch-knob" />
            </button>
          </div>
        </div>

        <div class="field-group">
          <div class="field-label">润色模型</div>
          <select class="select-input" v-model="polishForm.provider_id" @change="polishForm.model_id = ''">
            <option value="">— 选择服务商 —</option>
            <option v-for="p in ai.settings.providers.filter(p => p.enabled)" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
          <select class="select-input" style="margin-top:6px" v-model="polishForm.model_id" :disabled="!polishForm.provider_id">
            <option value="">— 选择模型（留空使用默认）—</option>
            <option
              v-for="m in ai.settings.providers.find(p => p.id === polishForm.provider_id)?.models ?? []"
              :key="m.id"
              :value="m.id"
            >{{ m.display_name }}</option>
          </select>
        </div>

        <div class="field-group">
          <div class="field-label">润色提示词</div>
          <textarea
            class="text-input mono"
            v-model="polishForm.prompt"
            rows="7"
            style="resize:vertical;line-height:1.5"
          />
          <div class="field-note">AI 会将 Codex 原始输出追加在提示词后发送。建议只调整格式，不修改内容。</div>
        </div>

        <div class="action-row">
          <button class="btn-primary" :disabled="polishSaving" @click="savePolish">
            {{ polishSaving ? '保存中…' : '保存' }}
          </button>
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
  flex: 1 1 48%;
  border-bottom: 1px solid var(--border-subtle);
}

.cli-source-section {
  flex: 1 1 52%;
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

.list-actions {
  display: flex;
  align-items: center;
  gap: 2px;
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

.mini-badge {
  display: inline-flex;
  align-items: center;
  min-height: 18px;
  max-width: 100%;
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  font-size: 10px;
  line-height: 1.3;
  font-weight: 600;
  white-space: nowrap;
}

.badge-ok {
  color: #2e7d32;
  background: color-mix(in srgb, #2e7d32 13%, transparent);
}

.badge-warn {
  color: #c56a10;
  background: color-mix(in srgb, #f59e0b 16%, transparent);
}

.badge-muted {
  color: var(--text-tertiary);
  background: var(--bg-tertiary);
}

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

.status-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: -8px 0 16px;
}

.version-text {
  color: var(--text-tertiary);
  font-weight: 500;
}

.not-detected-hint {
  font-size: 12px;
  line-height: 1.5;
  color: #c56a10;
  padding: 8px 10px;
  background: color-mix(in srgb, #f59e0b 12%, transparent);
  border: 1px solid color-mix(in srgb, #f59e0b 18%, transparent);
  border-radius: var(--radius-md);
  margin-bottom: 16px;
}

.danger-zone {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 22px;
  padding-top: 16px;
  border-top: 1px solid var(--border-subtle);
}

.saving-text {
  font-size: 12px;
  color: var(--text-tertiary);
}

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

.polish-item {
  border-top: 1px solid var(--border-subtle);
  margin-top: 4px;
  padding-top: 4px;
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
