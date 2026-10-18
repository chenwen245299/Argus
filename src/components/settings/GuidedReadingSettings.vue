<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useGuidedReadingStore } from '../../stores/guidedReading'
import { guidedReadingPrompt } from '../../utils/guidedReading'
import { BUILTIN_GUIDED_READING_ID, type GuidedReadingSettings } from '../../types/guidedReading'

const { t, locale } = useI18n()
const store = useGuidedReadingStore()
const draft = ref<GuidedReadingSettings>({ presets: [], defaultId: BUILTIN_GUIDED_READING_ID })
const original = ref(JSON.stringify(draft.value))
const selectedId = ref(BUILTIN_GUIDED_READING_ID)
const editingRoot = ref<string | null>(null)
const loading = ref(true), saving = ref(false), error = ref(''), saved = ref(false)
const dirty = computed(() => JSON.stringify(draft.value) !== original.value)
const builtin = computed(() => ({ id: BUILTIN_GUIDED_READING_ID, name: t('guidedReadingSettings.builtin'), prompt: guidedReadingPrompt(locale.value) }))
const choices = computed(() => [builtin.value, ...draft.value.presets])
const selected = computed(() => choices.value.find(p => p.id === selectedId.value) ?? builtin.value)
const isBuiltin = computed(() => selected.value.id === BUILTIN_GUIDED_READING_ID)
const valid = computed(() => draft.value.presets.every(p => p.name.trim() && p.prompt.trim()))
let disposed = false, epoch = 0
const offs: UnlistenFn[] = []

async function load() {
  const current = ++epoch
  loading.value = true; error.value = ''; saved.value = false
  try {
    if (!await store.load() || disposed || current !== epoch) return
    editingRoot.value = store.root
    draft.value = JSON.parse(JSON.stringify(store.settings))
    original.value = JSON.stringify(draft.value)
    if (!choices.value.some(p => p.id === selectedId.value)) selectedId.value = draft.value.defaultId
  } catch { if (!disposed && current === epoch) error.value = t('guidedReadingSettings.loadFailed') }
  finally { if (!disposed && current === epoch) loading.value = false }
}

function add() {
  const id = crypto.randomUUID()
  draft.value.presets.push({ id, name: t('guidedReadingSettings.newName', { n: draft.value.presets.length + 1 }), prompt: selected.value.prompt })
  selectedId.value = id; saved.value = false
}
function remove() {
  if (isBuiltin.value) return
  const id = selected.value.id
  draft.value.presets = draft.value.presets.filter(p => p.id !== id)
  if (draft.value.defaultId === id) draft.value.defaultId = BUILTIN_GUIDED_READING_ID
  selectedId.value = draft.value.defaultId; saved.value = false
}
async function save() {
  if (!valid.value || !editingRoot.value || saving.value) return
  saving.value = true; error.value = ''; saved.value = false
  const current = epoch
  try {
    await store.save(draft.value, editingRoot.value)
    if (!disposed && current === epoch) { original.value = JSON.stringify(draft.value); saved.value = true }
  } catch { if (!disposed && current === epoch) error.value = t('guidedReadingSettings.saveFailed') }
  finally { if (!disposed && current === epoch) saving.value = false }
}
function refresh() { if (!dirty.value && !saving.value) void load() }
onMounted(async () => {
  for (const [event, callback] of [
    ['library-changed', () => { saving.value = false; editingRoot.value = null; void load() }],
    ['guided-reading-settings-changed', refresh],
  ] as const) {
    const off = await listen(event, callback)
    if (disposed) off(); else offs.push(off)
  }
  if (!disposed) { window.addEventListener('focus', refresh); await load() }
})
onUnmounted(() => { disposed = true; epoch++; offs.forEach(off => off()); window.removeEventListener('focus', refresh) })
</script>

<template>
  <div class="reading-settings">
    <div class="reading-actions">
      <label class="default-choice">{{ t('guidedReadingSettings.default') }}
        <select v-model="draft.defaultId" :aria-label="t('guidedReadingSettings.default')" :disabled="loading || saving">
          <option v-for="preset in choices" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
        </select>
      </label>
      <button class="add-preset" :disabled="loading || saving" @click="add">
        <Icon icon="fluent:add-20-regular" width="16" />{{ t('guidedReadingSettings.add') }}
      </button>
    </div>
    <p v-if="error" class="reading-error" role="alert">{{ error }} <button v-if="!editingRoot" @click="load">{{ t('guidedReadingSettings.retry') }}</button></p>
    <p v-if="loading" role="status">{{ t('guidedReadingSettings.loading') }}</p>
    <div v-else class="reading-editor">
      <div class="preset-list" role="list" :aria-label="t('guidedReadingSettings.presets')">
        <button v-for="preset in choices" :key="preset.id" class="preset-row" :class="{ selected: selectedId === preset.id }"
          :aria-pressed="selectedId === preset.id" :disabled="saving" @click="selectedId = preset.id; saved = false">
          <span>{{ preset.name }}</span><span v-if="draft.defaultId === preset.id" class="default-badge">{{ t('guidedReadingSettings.defaultBadge') }}</span>
        </button>
      </div>
      <div class="preset-form">
        <label>{{ t('guidedReadingSettings.name') }}<input v-model="selected.name" :readonly="isBuiltin" :disabled="saving" /></label>
        <label class="prompt-field">{{ t('guidedReadingSettings.prompt') }}
          <textarea v-model="selected.prompt" :readonly="isBuiltin" :disabled="saving" spellcheck="false" />
        </label>
        <div class="preset-footer">
          <span v-if="isBuiltin" class="preset-hint">{{ t('guidedReadingSettings.builtinHint') }}</span>
          <button v-else class="delete-preset" :disabled="saving" @click="remove">{{ t('guidedReadingSettings.delete') }}</button>
        </div>
      </div>
    </div>
    <div class="save-row">
      <span v-if="!valid" class="reading-error" role="status">{{ t('guidedReadingSettings.required') }}</span>
      <span v-else-if="saved && !dirty" class="saved-state" role="status">{{ t('guidedReadingSettings.saved') }}</span>
      <span v-else-if="dirty" class="preset-hint">{{ t('guidedReadingSettings.unsaved') }}</span>
      <button class="save-presets" :disabled="loading || saving || !dirty || !valid || !editingRoot" @click="save">{{ t(saving ? 'guidedReadingSettings.saving' : 'guidedReadingSettings.save') }}</button>
    </div>
  </div>
</template>

<style scoped>
.reading-settings { display: flex; flex-direction: column; gap: 14px; min-height: 0; }
.reading-actions, .save-row { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; }
.default-choice { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; font-size: 13px; color: var(--text-secondary); }
select, input, textarea { box-sizing: border-box; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); padding: 8px 10px; background: var(--bg-primary); color: var(--text-primary); font: inherit; }
select { max-width: 260px; min-width: 160px; }
input:focus, textarea:focus, select:focus { outline: 2px solid var(--accent); outline-offset: 1px; }
.add-preset, .save-presets { display: inline-flex; align-items: center; justify-content: center; gap: 5px; padding: 7px 12px; border-radius: var(--radius-sm); font-size: 12px; }
.add-preset { margin-left: auto; color: var(--accent); background: var(--bg-hover); }
.save-presets { margin-left: auto; background: var(--accent); color: #fff; }
button:disabled { opacity: .45; cursor: default; }
.reading-editor { display: grid; grid-template-columns: 180px minmax(0, 1fr); min-height: 400px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; background: var(--bg-primary); }
.preset-list { padding: 8px; border-right: 1px solid var(--border-subtle); background: var(--bg-secondary); max-height: 620px; overflow-y: auto; }
.preset-row { display: flex; align-items: center; gap: 6px; width: 100%; padding: 10px 8px; border-radius: var(--radius-sm); color: var(--text-secondary); font-size: 12px; text-align: left; }
.preset-row > span:first-child { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.preset-row:hover { background: var(--bg-hover); }
.preset-row.selected { background: color-mix(in srgb, var(--accent) 12%, transparent); color: var(--accent); }
.default-badge { font-size: 10px; flex-shrink: 0; }
.preset-form { display: flex; flex-direction: column; gap: 14px; padding: 16px; min-width: 0; }
.preset-form label { display: flex; flex-direction: column; gap: 7px; font-size: 12px; color: var(--text-secondary); }
.prompt-field { flex: 1; }
.prompt-field textarea { width: 100%; min-height: 330px; resize: vertical; font-size: 12px; line-height: 1.65; }
[readonly] { color: var(--text-secondary); }
.preset-footer { min-height: 24px; }
.preset-hint, .saved-state, .reading-error { font-size: 12px; line-height: 1.5; }
.preset-hint { color: var(--text-tertiary); }
.saved-state { color: var(--accent); }
.reading-error, .delete-preset { color: var(--danger, #c44); }
.delete-preset { font-size: 12px; padding: 3px 0; }
@media (max-width: 760px) {
  .reading-editor { grid-template-columns: 1fr; }
  .preset-list { display: flex; gap: 4px; max-height: 140px; border-right: 0; border-bottom: 1px solid var(--border-subtle); overflow: auto; }
  .preset-row { width: auto; min-width: 110px; max-width: 200px; flex-shrink: 0; }
}
</style>
