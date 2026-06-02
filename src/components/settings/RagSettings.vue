<script setup lang="ts">
import { ref, computed, onMounted, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useRagStore } from '../../stores/rag'
import { useAiStore } from '../../stores/ai'
import type { RagSettings, PaperIndexEntry, PaperVectorizeInput, ChunkInput, AiModel } from '../../types'
import { buildChunks } from '../../utils/chunker'

const { t } = useI18n()
const ragStore = useRagStore()
const aiStore = useAiStore()

const form = ref<RagSettings>({ ...ragStore.settings })
const saving = ref(false)
const saveMsg = ref('')
const rebuilding = ref(false)
const rebuildMsg = ref('')
const rebuildProgress = ref({ done: 0, total: 0, failed: 0 })
const rebuildCurrentPaper = ref('')
let cancelRequested = false
let formReady = false
let skipAutoSave = false
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null

onMounted(async () => {
  await ragStore.load()
  await ragStore.loadStoreInfo()
  await aiStore.load()
  form.value = { ...ragStore.settings }
  await nextTick()
  formReady = true
})

watch(() => ragStore.settings, (s) => {
  if (!skipAutoSave) form.value = { ...s }
}, { deep: true })

watch(form, () => {
  if (!formReady || skipAutoSave) return
  if (autoSaveTimer) clearTimeout(autoSaveTimer)
  autoSaveTimer = setTimeout(() => save(), 600)
}, { deep: true })

type EmbeddingModelOption = {
  providerId: string
  providerName: string
  modelId: string
  displayName: string
}

function embeddingModelKey(model: EmbeddingModelOption) {
  return JSON.stringify([model.providerId, model.modelId])
}

function parseEmbeddingModelKey(key: string) {
  try {
    const parsed = JSON.parse(key)
    if (!Array.isArray(parsed) || parsed.length !== 2) return null
    const [providerId, modelId] = parsed
    if (typeof providerId !== 'string' || typeof modelId !== 'string') return null
    return { providerId, modelId }
  } catch {
    return null
  }
}

function modelSearchText(model: AiModel) {
  return `${model.display_name ?? ''} ${model.id ?? ''} ${(model.capabilities ?? []).join(' ')}`.toLowerCase()
}

function modelHasEmbeddingCapability(model: AiModel) {
  const caps = new Set((model.capabilities ?? []).filter(Boolean))
  const text = modelSearchText(model)
  return (
    caps.has('embedding') ||
    caps.has('embed') ||
    caps.has('embeddings') ||
    /\b(embed|embedding|embeddings)\b/.test(text) ||
    /text-embedding|bge-|gte-|e5-|voyage-/.test(text)
  )
}

const embeddingModels = computed<EmbeddingModelOption[]>(() =>
  (aiStore.settings.providers ?? [])
    .filter(provider => provider.kind !== 'anthropic' && provider.enabled && provider.has_key)
    .flatMap(provider =>
      (provider.models ?? [])
        .filter(model => model.enabled !== false && modelHasEmbeddingCapability(model))
        .map(model => ({
          providerId: provider.id,
          providerName: provider.name,
          modelId: model.id,
          displayName: model.display_name || model.id,
        }))
    )
)

const groupedEmbeddingModels = computed(() => {
  const groups = new Map<string, { id: string; name: string; models: EmbeddingModelOption[] }>()
  for (const model of embeddingModels.value) {
    if (!groups.has(model.providerId)) {
      groups.set(model.providerId, { id: model.providerId, name: model.providerName, models: [] })
    }
    groups.get(model.providerId)!.models.push(model)
  }
  return Array.from(groups.values())
})

const selectedEmbeddingModelKey = computed({
  get() {
    if (!form.value.provider_id || !form.value.embedding_model) return ''
    const key = JSON.stringify([form.value.provider_id, form.value.embedding_model])
    return embeddingModels.value.some(model => embeddingModelKey(model) === key) ? key : ''
  },
  set(key: string) {
    const parsed = parseEmbeddingModelKey(key)
    form.value.provider_id = parsed?.providerId ?? null
    form.value.embedding_model = parsed?.modelId ?? null
  },
})

const hasUnavailableEmbeddingModel = computed(() =>
  !!form.value.provider_id &&
  !!form.value.embedding_model &&
  !embeddingModels.value.some(model =>
    model.providerId === form.value.provider_id && model.modelId === form.value.embedding_model
  )
)

async function save() {
  if (saving.value) return
  saving.value = true
  saveMsg.value = ''
  try {
    skipAutoSave = true
    await ragStore.save(form.value)
    await nextTick()
    skipAutoSave = false
    saveMsg.value = t('ragSettings.saved')
    setTimeout(() => saveMsg.value = '', 2000)
  } catch (e) {
    saveMsg.value = String(e)
  } finally {
    saving.value = false
  }
}

// mode='full' — embed every paper regardless of vectorized status
// mode='missing' — only embed papers not yet vectorized (断点续建 & 增量同步)
async function rebuild(mode: 'full' | 'missing') {
  rebuilding.value = true
  cancelRequested = false
  rebuildMsg.value = ''
  rebuildCurrentPaper.value = ''
  rebuildProgress.value = { done: 0, total: 0, failed: 0 }

  try {
    const allPapers = await invoke<PaperIndexEntry[]>('list_papers')
    const papers = mode === 'missing'
      ? allPapers.filter(p => !p.status.vectorized)
      : allPapers

    const total = papers.length
    let done = 0, failed = 0
    rebuildProgress.value = { done, total, failed }

    if (total === 0) {
      rebuildMsg.value = mode === 'missing' ? '所有论文已同步，无需处理' : '暂无论文'
      return
    }

    const chunkSize: number = form.value.chunk_size ?? 512
    const chunkOverlap: number = form.value.chunk_overlap ?? 50

    for (const paper of papers) {
      if (cancelRequested) break
      rebuildCurrentPaper.value = paper.title

      try {
        const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: paper.slug })
        const chunks: ChunkInput[] = buildChunks(input, chunkSize, chunkOverlap)
        if (chunks.length === 0) { failed++; rebuildProgress.value = { done, total, failed }; continue }
        await invoke('embed_and_store_chunks', {
          slug: paper.slug,
          paperId: input.paper_id,
          paperTitle: input.paper_title,
          chunks,
        })
        done++
      } catch {
        failed++
      }

      rebuildProgress.value = { done, total, failed }
    }

    if (cancelRequested) {
      rebuildMsg.value = `已暂停，已完成 ${done}/${total}。点击「同步缺失」可从断点继续。`
    } else {
      rebuildMsg.value = `完成 ${done}/${total} 篇${failed > 0 ? `，${failed} 篇失败` : ''}`
    }
    await ragStore.loadStoreInfo()
  } catch (e) {
    rebuildMsg.value = String(e)
  } finally {
    rebuilding.value = false
    rebuildCurrentPaper.value = ''
  }
}

function cancelRebuild() {
  cancelRequested = true
}
</script>

<template>
  <div class="rag-settings">
    <h2 class="section-title">{{ t('ragSettings.title') }}</h2>
    <p class="section-desc">{{ t('ragSettings.desc') }}</p>

    <!-- Enable toggle -->
    <div class="field-row">
      <label class="field-label">{{ t('ragSettings.enabled') }}</label>
      <label class="toggle">
        <input type="checkbox" v-model="form.enabled" />
        <span class="toggle-track" />
      </label>
    </div>

    <!-- Embedding model -->
    <div class="field-group">
      <label class="field-label">{{ t('ragSettings.embeddingModel') }}</label>
      <select
        class="field-input"
        v-model="selectedEmbeddingModelKey"
        :disabled="!form.enabled || embeddingModels.length === 0"
      >
        <option value="">
          {{ embeddingModels.length ? '选择已启用的嵌入模型' : '暂无可用的嵌入模型' }}
        </option>
        <optgroup v-for="group in groupedEmbeddingModels" :key="group.id" :label="group.name">
          <option
            v-for="model in group.models"
            :key="embeddingModelKey(model)"
            :value="embeddingModelKey(model)"
          >
            {{ model.displayName }} · {{ model.modelId }}
          </option>
        </optgroup>
      </select>
      <p v-if="hasUnavailableEmbeddingModel" class="field-warning">
        当前嵌入模型未启用或不支持 embedding，请从已启用模型中重新选择。
      </p>
      <p v-else class="field-hint">从 AI 供应商中已启用的 embedding 模型里选择。</p>
    </div>

    <!-- Chunk size -->
    <div class="field-row">
      <label class="field-label">{{ t('ragSettings.chunkSize') }} <span class="unit-hint">(tokens)</span></label>
      <input class="field-input sm" type="number" v-model.number="form.chunk_size" min="128" max="2048" step="64" :disabled="!form.enabled" />
    </div>

    <!-- Chunk overlap -->
    <div class="field-row">
      <label class="field-label">{{ t('ragSettings.chunkOverlap') }} <span class="unit-hint">(tokens)</span></label>
      <input class="field-input sm" type="number" v-model.number="form.chunk_overlap" min="0" max="512" step="32" :disabled="!form.enabled" />
    </div>

    <!-- Top K -->
    <div class="field-row">
      <label class="field-label">{{ t('ragSettings.topK') }}</label>
      <input class="field-input sm" type="number" v-model.number="form.top_k" min="1" max="20" :disabled="!form.enabled" />
    </div>

    <!-- Save button -->
    <div class="action-row">
      <button class="btn-primary" @click="save" :disabled="saving">
        {{ saving ? t('ragSettings.saving') : t('ragSettings.save') }}
      </button>
      <span v-if="saveMsg" class="save-msg">{{ saveMsg }}</span>
    </div>

    <!-- Vector store info -->
    <div class="store-info" v-if="ragStore.storeInfo">
      <h3 class="store-title">{{ t('ragSettings.storeInfo') }}</h3>
      <div class="info-grid">
        <span class="info-label">{{ t('ragSettings.totalChunks') }}</span>
        <span class="info-val">{{ ragStore.storeInfo.total_chunks }}</span>
        <span class="info-label">{{ t('ragSettings.uniquePapers') }}</span>
        <span class="info-val">{{ ragStore.storeInfo.unique_papers }}</span>
        <span class="info-label">{{ t('ragSettings.dimension') }}</span>
        <span class="info-val">{{ ragStore.storeInfo.dimension ?? '—' }}</span>
        <span class="info-label">{{ t('ragSettings.storeModel') }}</span>
        <span class="info-val">{{ ragStore.storeInfo.embedding_model ?? '—' }}</span>
      </div>
    </div>

    <!-- Rebuild -->
    <div class="rebuild-section">
      <h3 class="store-title">向量库管理</h3>
      <p class="field-hint">「同步缺失」只处理尚未向量化的论文，支持断点续建和增量添加；「完整重建」重新处理所有论文。</p>
      <div class="rebuild-controls">
        <button class="btn-primary sm" @click="rebuild('missing')" :disabled="rebuilding || !form.enabled">
          同步缺失
        </button>
        <button class="btn-danger sm" @click="rebuild('full')" :disabled="rebuilding || !form.enabled">
          完整重建
        </button>
        <button v-if="rebuilding" class="btn-ghost sm" @click="cancelRebuild">
          {{ t('ragSettings.cancelBtn') }}
        </button>
      </div>
      <div v-if="rebuilding && rebuildProgress.total > 0" class="progress-wrap">
        <div class="progress-bar-wrap">
          <div
            class="progress-bar"
            :style="{ width: (rebuildProgress.done / rebuildProgress.total * 100) + '%' }"
          />
        </div>
        <div class="progress-meta">
          <span class="progress-count">{{ rebuildProgress.done }}/{{ rebuildProgress.total }}
            <template v-if="rebuildProgress.failed > 0">（{{ rebuildProgress.failed }} 失败）</template>
          </span>
          <span v-if="rebuildCurrentPaper" class="progress-paper" :title="rebuildCurrentPaper">
            {{ rebuildCurrentPaper }}
          </span>
        </div>
      </div>
      <p v-if="rebuildMsg" class="rebuild-msg">{{ rebuildMsg }}</p>
    </div>
  </div>
</template>

<style scoped>
.rag-settings { padding: 24px 28px; display: flex; flex-direction: column; gap: 14px; overflow-y: auto; }
.section-title { font-size: var(--font-size-lg); font-weight: 600; margin-bottom: 2px; }
.section-desc { font-size: var(--font-size-sm); color: var(--text-secondary); }
.field-group { display: flex; flex-direction: column; gap: 5px; }
.field-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.field-label { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-primary); }
.field-input { padding: 6px 9px; font-size: var(--font-size-sm); border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-primary); color: var(--text-primary); width: 100%; }
.field-input.sm { width: 80px; }
.field-input:disabled { opacity: 0.5; }
.field-hint { font-size: var(--font-size-xs); color: var(--text-tertiary); line-height: 1.4; }
.field-warning { font-size: var(--font-size-xs); color: #c56a10; line-height: 1.4; }
.unit-hint { font-size: var(--font-size-xs); color: var(--text-tertiary); font-weight: 400; }
.action-row { display: flex; align-items: center; gap: 10px; }
.save-msg { font-size: var(--font-size-xs); color: var(--accent); }
.toggle { display: inline-flex; align-items: center; cursor: pointer; }
.toggle input { display: none; }
.toggle-track { width: 36px; height: 20px; background: var(--border-default); border-radius: 10px; position: relative; transition: background 0.15s; }
.toggle input:checked + .toggle-track { background: var(--accent); }
.toggle-track::after { content: ''; position: absolute; width: 14px; height: 14px; border-radius: 50%; background: #fff; top: 3px; left: 3px; transition: left 0.15s; }
.toggle input:checked + .toggle-track::after { left: 19px; }
.store-info { background: var(--bg-secondary); border-radius: var(--radius-md); padding: 14px 16px; }
.store-title { font-size: var(--font-size-sm); font-weight: 600; margin-bottom: 10px; }
.info-grid { display: grid; grid-template-columns: auto 1fr; gap: 5px 16px; }
.info-label { font-size: var(--font-size-xs); color: var(--text-secondary); }
.info-val { font-size: var(--font-size-xs); font-weight: 500; }
.rebuild-section { border-top: 1px solid var(--border-subtle); padding-top: 14px; display: flex; flex-direction: column; gap: 8px; }
.rebuild-controls { display: flex; gap: 8px; flex-wrap: wrap; }
.progress-wrap { display: flex; flex-direction: column; gap: 6px; }
.progress-bar-wrap { height: 6px; background: var(--bg-tertiary); border-radius: 3px; overflow: hidden; }
.progress-bar { height: 100%; background: var(--accent); transition: width 0.25s; border-radius: 3px; }
.progress-meta { display: flex; align-items: baseline; gap: 10px; }
.progress-count { font-size: 11px; color: var(--text-secondary); white-space: nowrap; flex-shrink: 0; }
.progress-paper { font-size: 11px; color: var(--text-tertiary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.rebuild-msg { font-size: var(--font-size-xs); color: var(--text-secondary); }
.btn-primary { padding: 6px 14px; font-size: var(--font-size-sm); font-weight: 500; background: var(--accent); color: #fff; border-radius: var(--radius-sm); }
.btn-primary:hover:not(:disabled) { background: var(--accent-hover); }
.btn-primary:disabled { opacity: 0.4; cursor: not-allowed; }
.btn-primary.sm { padding: 4px 10px; font-size: var(--font-size-xs); }
.btn-danger { padding: 6px 14px; font-size: var(--font-size-sm); font-weight: 500; background: #cc3333; color: #fff; border-radius: var(--radius-sm); }
.btn-danger:hover:not(:disabled) { background: #aa2222; }
.btn-danger:disabled { opacity: 0.4; cursor: not-allowed; }
.btn-danger.sm { padding: 4px 10px; font-size: var(--font-size-xs); }
.btn-ghost { padding: 4px 10px; font-size: var(--font-size-xs); color: var(--text-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); }
.btn-ghost:hover { background: var(--bg-hover); }
.btn-ghost.sm { padding: 3px 8px; font-size: 11px; }
</style>
