<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useRagStore, sameRagSettings, type VectorRebuildMode } from '../../stores/rag'
import { useAiStore } from '../../stores/ai'
import type { RagSettings, AiModel } from '../../types'

const { t } = useI18n()
const ragStore = useRagStore()
const aiStore = useAiStore()

const form = ref<RagSettings>({ ...ragStore.settings })
const saving = ref(false)
const saveMsg = ref('')
let formReady = false
let skipAutoSave = false
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null
let unlistenRagSettings: UnlistenFn | null = null
let unmounted = false

onMounted(async () => {
  // A save from another window (or this one) is broadcast as
  // `rag-settings-changed`. The main window reloads its store on it, but the
  // chat and embedding-map windows only have this panel to do it. The reload
  // cannot start a save loop: `load()` keeps the store's object when nothing
  // changed (so neither watcher fires), and when something did, the form takes
  // the new values and its watcher finds them equal to the store and returns.
  void listen('rag-settings-changed', () => {
    void ragStore.load()
    void ragStore.loadStoreInfo()
  }).then((off) => {
    if (unmounted) off()
    else unlistenRagSettings = off
  }).catch(() => {})

  await ragStore.load()
  await ragStore.loadStoreInfo()
  await aiStore.load()
  form.value = { ...ragStore.settings }
  await nextTick()
  formReady = true
})

// No cancel here: the vector run lives in the store and carries on. A pending
// auto-save is left to fire too, so an edit made just before closing is kept.
onUnmounted(() => {
  unmounted = true
  unlistenRagSettings?.()
  unlistenRagSettings = null
})

watch(() => ragStore.settings, (s) => {
  if (!skipAutoSave) form.value = { ...s }
}, { deep: true })

watch(form, () => {
  if (!formReady || skipAutoSave) return
  if (autoSaveTimer) { clearTimeout(autoSaveTimer); autoSaveTimer = null }
  // A form that only caught up with the store (a reload, or a save made in
  // another window) has nothing to write. Saving it anyway would broadcast
  // `rag-settings-changed` for a change that is not one.
  if (sameRagSettings(form.value, ragStore.settings)) return
  scheduleAutoSave()
}, { deep: true })

function scheduleAutoSave() {
  if (autoSaveTimer) clearTimeout(autoSaveTimer)
  autoSaveTimer = setTimeout(() => { autoSaveTimer = null; void save() }, 600)
}

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
    .filter(provider => provider.kind !== 'anthropic' && provider.enabled && (provider.has_key || provider.kind === 'ollama'))
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
  // Cancel any pending debounced auto-save so a manual click doesn't fire a
  // duplicate IPC write right after.
  if (autoSaveTimer) { clearTimeout(autoSaveTimer); autoSaveTimer = null }
  saving.value = true
  saveMsg.value = ''
  let saved = false
  try {
    skipAutoSave = true
    await ragStore.save(form.value)
    // Let the store watcher run while it is still skipped, so the stored copy
    // does not overwrite an edit made during the write.
    await nextTick()
    saved = true
    saveMsg.value = t('ragSettings.saved')
    setTimeout(() => saveMsg.value = '', 2000)
  } catch (e) {
    saveMsg.value = String(e)
  } finally {
    // Always, or one failed save would leave auto-save off for the session.
    skipAutoSave = false
    saving.value = false
  }
  // An edit made while the write was in flight was skipped by the form
  // watcher, and the store only recorded what was written: save it now.
  // Not after a failure — that would retry on a loop; the next edit retries.
  if (saved && !sameRagSettings(form.value, ragStore.settings)) scheduleAutoSave()
}

// The 同步缺失 / 完整重建 run itself lives in the RAG store, so it survives
// this panel being closed or switched away from; the panel only drives it and
// shows the store's progress, reattaching when it is mounted again.
// mode='full' — embed every paper regardless of vectorized status
// mode='missing' — only embed papers not yet vectorized (断点续建 & 增量同步)
function rebuild(mode: VectorRebuildMode) {
  if (ragStore.rebuilding) return
  deleteMsg.value = ''
  void ragStore.rebuildVectors(mode, form.value.chunk_size, form.value.chunk_overlap)
}

function cancelRebuild() {
  ragStore.cancelRebuild()
}

/** A model-deletion error, shown on the run's message line until the next run. */
const deleteMsg = ref('')

const rebuildMsg = computed(() => {
  if (deleteMsg.value) return deleteMsg.value
  const outcome = ragStore.rebuildOutcome
  if (!outcome) return ''
  switch (outcome.kind) {
    case 'nothing':
      return outcome.mode === 'missing' ? t('ragSettings.allSynced') : t('ragSettings.noPapers')
    case 'paused':
      return t('ragSettings.rebuildPaused', { done: outcome.done, total: outcome.total })
    case 'done':
      return outcome.failed > 0
        ? t('ragSettings.rebuildDoneWithFailed', { done: outcome.done, total: outcome.total, failed: outcome.failed })
        : t('ragSettings.rebuildDoneCount', { done: outcome.done, total: outcome.total })
    case 'error':
      return outcome.message
  }
  return ''
})

// Per-model deletion: drops one model's whole partition, leaving others intact.
const deletingModel = ref('')
async function deleteModelEmbeddings(model: string) {
  if (deletingModel.value) return
  if (!window.confirm(t('ragSettings.confirmDeleteModel', { model }))) return
  deletingModel.value = model
  try {
    await invoke('delete_model_embeddings', { model })
    await ragStore.loadStoreInfo()
  } catch (e) {
    deleteMsg.value = String(e)
  } finally {
    deletingModel.value = ''
  }
}
</script>

<template>
  <div class="rag-settings">

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
          {{ embeddingModels.length ? t('ragSettings.selectEmbeddingModel') : t('ragSettings.noEmbeddingModels') }}
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
        {{ t('ragSettings.embeddingModelInvalid') }}
      </p>
      <p v-else class="field-hint">{{ t('ragSettings.embeddingModelSource') }}</p>
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

    <!-- No top-k field: it sized chat retrieval, and chat no longer retrieves.
         `form.top_k` is still round-tripped untouched, so the saved setting
         survives for anything that reads it. -->

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

      <!-- Stored models: each embedding model keeps its own vectors -->
      <div class="model-list" v-if="ragStore.storeInfo.models && ragStore.storeInfo.models.length">
        <div class="model-list-title">{{ t('ragSettings.storedModels') }}</div>
        <div
          v-for="m in ragStore.storeInfo.models"
          :key="m.embedding_model"
          class="model-row"
          :class="{ 'is-current': m.embedding_model === ragStore.storeInfo.embedding_model }"
        >
          <div class="model-meta">
            <span class="model-name" :title="m.embedding_model">{{ m.embedding_model }}</span>
            <span class="model-sub">{{ t('ragSettings.modelStat', { dim: m.dimension, papers: m.unique_papers, chunks: m.total_chunks }) }}</span>
          </div>
          <button
            class="btn-ghost sm"
            :disabled="deletingModel === m.embedding_model"
            @click="deleteModelEmbeddings(m.embedding_model)"
          >
            {{ deletingModel === m.embedding_model ? t('ragSettings.deleting') : t('ragSettings.cleanModel') }}
          </button>
        </div>
      </div>
    </div>

    <!-- Rebuild -->
    <div class="rebuild-section">
      <h3 class="store-title">{{ t('ragSettings.storeManage') }}</h3>
      <p class="field-hint">{{ t('ragSettings.storeManageHint') }}</p>
      <div class="rebuild-controls">
        <button class="btn-primary sm" @click="rebuild('missing')" :disabled="ragStore.rebuilding || !form.enabled">
          {{ t('ragSettings.syncMissing') }}
        </button>
        <button class="btn-danger sm" @click="rebuild('full')" :disabled="ragStore.rebuilding || !form.enabled">
          {{ t('ragSettings.fullRebuild') }}
        </button>
        <button v-if="ragStore.rebuilding" class="btn-ghost sm" @click="cancelRebuild">
          {{ t('ragSettings.cancelBtn') }}
        </button>
      </div>
      <div v-if="ragStore.rebuilding && ragStore.rebuildProgress.total > 0" class="progress-wrap">
        <div class="progress-bar-wrap">
          <div
            class="progress-bar"
            :style="{ width: (ragStore.rebuildProgress.done / ragStore.rebuildProgress.total * 100) + '%' }"
          />
        </div>
        <div class="progress-meta">
          <span class="progress-count">{{ ragStore.rebuildProgress.done }}/{{ ragStore.rebuildProgress.total }}
            <template v-if="ragStore.rebuildProgress.failed > 0">{{ t('ragSettings.failedCount', { n: ragStore.rebuildProgress.failed }) }}</template>
          </span>
          <span v-if="ragStore.rebuildCurrentPaper" class="progress-paper" :title="ragStore.rebuildCurrentPaper">
            {{ ragStore.rebuildCurrentPaper }}
          </span>
        </div>
      </div>
      <p v-if="rebuildMsg" class="rebuild-msg">{{ rebuildMsg }}</p>
    </div>
  </div>
</template>

<style scoped>
.rag-settings { padding-bottom: 8px; display: flex; flex-direction: column; gap: 14px; }
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
.model-list { margin-top: 12px; border-top: 1px solid var(--border-subtle); padding-top: 10px; display: flex; flex-direction: column; gap: 6px; }
.model-list-title { font-size: var(--font-size-xs); font-weight: 600; color: var(--text-secondary); }
.model-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; padding: 6px 8px; border-radius: var(--radius-sm); background: var(--bg-primary); }
.model-row.is-current { outline: 1px solid color-mix(in srgb, var(--accent) 45%, transparent); }
.model-meta { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.model-name { font-size: var(--font-size-xs); font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.model-sub { font-size: 11px; color: var(--text-tertiary); }
.btn-ghost.sm { flex-shrink: 0; }
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
