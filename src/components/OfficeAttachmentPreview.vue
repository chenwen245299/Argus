<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { buildContentParts, type Attachment } from '../utils/attachments'
const props = defineProps<{ attachment: Attachment; expectedRoot: string | null }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()
const dialog = ref<HTMLElement | null>(null)
const text = ref(''), loading = ref(false), error = ref('')
const sheets = ref<{ name: string }[]>([]), sheet = ref('')
const row = ref(1), column = ref(1), range = ref('')
const nextOffset = ref<number | null>(null), nextRow = ref<number | null>(null), nextColumn = ref<number | null>(null)
const isExcel = computed(() => props.attachment.type === 'excel')
const displayText = computed(() => !isExcel.value ? text.value : text.value.split('\n').map(line => {
  if (!line) return ''
  try {
    const cell = JSON.parse(line)
    const value = cell.value === null ? t('chat.officePreviewNoValue') : String(cell.value)
    return `${cell.cell}: ${value}${cell.formula ? `  [=${cell.formula}]` : ''}`
  } catch { return t('chat.officePreviewIncompleteCell') }
}).join('\n'))
interface Preview { text?: string; next_offset?: number | null; next_row?: number | null; next_column?: number | null; range?: string; sheets?: { name: string }[] }
let revision = 0
async function request(offset: number, view = 'text'): Promise<Preview> {
  if (!props.expectedRoot) throw new Error(t('chat.officePreviewNoLibrary'))
  return invoke('read_office_attachment_preview', {
    attachment: buildContentParts('', [props.attachment])[1], expectedRoot: props.expectedRoot, offset, view,
    ...(isExcel.value ? { sheet: sheet.value || null, startRow: row.value, startColumn: column.value } : {}),
  })
}
async function load(offset = 0) {
  const current = ++revision
  loading.value = true; error.value = ''
  try {
    if (isExcel.value && !sheets.value.length) {
      const overview = await request(0, 'overview')
      if (current !== revision) return
      sheets.value = overview.sheets ?? []; sheet.value = sheets.value[0]?.name ?? ''
    }
    const result = await request(offset)
    if (current !== revision) return
    text.value = offset ? text.value + (result.text ?? '') : result.text ?? ''
    nextOffset.value = result.next_offset ?? null; nextRow.value = result.next_row ?? null; nextColumn.value = result.next_column ?? null
    range.value = result.range ?? ''
  } catch (e) { if (current === revision) error.value = String(e) }
  finally { if (current === revision) loading.value = false }
}
function changeSheet() { row.value = 1; column.value = 1; void load() }
function nextRows() { if (nextRow.value !== null) { row.value = nextRow.value; void load() } }
function nextColumns() { if (nextColumn.value !== null) { column.value = nextColumn.value; void load() } }
watch(() => [props.attachment.id, props.expectedRoot], () => {
  text.value = ''; sheets.value = []; sheet.value = ''; row.value = 1; column.value = 1
  nextOffset.value = null; nextRow.value = null; nextColumn.value = null; void load()
}, { immediate: true })
onMounted(() => { dialog.value?.focus() })
onBeforeUnmount(() => { revision++ })
</script>

<template>
  <div class="office-preview-backdrop" @click.self="emit('close')" @keydown.esc="emit('close')">
    <section ref="dialog" class="office-preview" role="dialog" aria-modal="true" :aria-label="attachment.name" tabindex="-1">
      <header><strong>{{ attachment.name }}</strong><button :title="t('chat.officePreviewClose')" @click="emit('close')"><Icon icon="fluent:dismiss-24-regular" width="18" /></button></header>
      <p class="office-preview-hint">{{ t(attachment.type === 'json' ? 'chat.jsonPreviewHint' : 'chat.officePreviewHint') }}</p>
      <div v-if="isExcel && sheets.length" class="office-preview-grid-controls">
        <select v-model="sheet" :disabled="loading" :aria-label="t('chat.officePreviewSheet')" @change="changeSheet"><option v-for="item in sheets" :key="item.name" :value="item.name">{{ item.name }}</option></select>
        <span>{{ range }}</span>
      </div>
      <pre v-if="text">{{ displayText }}</pre>
      <p v-else-if="!loading && !error" class="office-preview-state">{{ t('chat.officePreviewEmpty') }}</p>
      <p v-if="error" class="office-preview-error">{{ error }}</p>
      <div class="office-preview-footer">
        <span v-if="loading">{{ t('chat.officePreviewLoading') }}</span>
        <button v-else-if="error" @click="load()">{{ t('chat.officePreviewRetry') }}</button>
        <button v-else-if="nextOffset !== null" @click="load(nextOffset)">{{ t('chat.officePreviewMore') }}</button>
        <template v-if="!loading && !error && nextOffset === null">
          <button v-if="nextRow !== null" @click="nextRows">{{ t('chat.officePreviewNextRows') }}</button>
          <button v-if="nextColumn !== null" @click="nextColumns">{{ t('chat.officePreviewNextColumns') }}</button>
        </template>
      </div>
    </section>
  </div>
</template>

<style scoped>
.office-preview-backdrop { position: absolute; inset: 0; z-index: 100; display: flex; align-items: center; justify-content: center; padding: 16px; background: rgba(0,0,0,.4); }
.office-preview { width: min(780px, 100%); max-height: 92%; min-width: 0; display: flex; flex-direction: column; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg, 12px); box-shadow: var(--shadow-lg); overflow: hidden; }
header { display: flex; align-items: center; gap: 12px; padding: 12px 16px; border-bottom: 1px solid var(--border-subtle); }
header strong { flex: 1; min-width: 0; overflow-wrap: anywhere; }
button { border: 0; padding: 6px 10px; border-radius: var(--radius-sm); background: var(--bg-secondary); color: var(--text-primary); cursor: pointer; font: inherit; }
button:hover { color: var(--accent); }
.office-preview-hint { margin: 10px 16px; font-size: 12px; color: var(--text-secondary); }
pre { white-space: pre-wrap; overflow-wrap: anywhere; overflow: auto; padding: 12px 16px; margin: 0; font: 13px/1.6 var(--font-mono, monospace); user-select: text; }
.office-preview-state, .office-preview-error { padding: 12px 16px; margin: 0; overflow-wrap: anywhere; }
.office-preview-error { color: var(--text-danger, var(--text-primary)); }
.office-preview-grid-controls { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; padding: 0 16px 8px; color: var(--text-secondary); font-size: 12px; }
select { min-width: 0; max-width: 100%; background: var(--bg-secondary); color: var(--text-primary); border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); padding: 5px; }
.office-preview-footer { display: flex; flex-wrap: wrap; gap: 8px; padding: 10px 16px; font-size: 12px; color: var(--text-secondary); }
</style>
