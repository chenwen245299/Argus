<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { KnowledgeLevel, MemoryCategory, UserMemory, UserMemoryEntry } from '../../types/userMemory'

const { t, locale } = useI18n()
const profile = ref<UserMemory | null>(null)
const loading = ref(true), busy = ref(false), error = ref('')
const editing = ref<UserMemoryEntry | null>(null), deleting = ref<string | null>(null)
const categories: MemoryCategory[] = ['knowledge', 'background', 'research', 'preferences', 'goals']
const levels: KnowledgeLevel[] = ['familiar', 'learning', 'new', 'unspecified']
const groups = computed(() => categories.map(category => ({ category, entries: profile.value?.entries.filter(e => e.category === category) ?? [] })).filter(g => g.entries.length))
let disposed = false, off: UnlistenFn | undefined, readEpoch = 0

function accept(value: UserMemory) {
  if (!disposed && (!profile.value || value.revision >= profile.value.revision)) profile.value = value
}
async function load() {
  const epoch = ++readEpoch
  try {
    const value = await invoke<UserMemory>('get_user_memory')
    if (!disposed && epoch === readEpoch) { accept(value); error.value = '' }
  } catch { if (!disposed && epoch === readEpoch) error.value = t('userMemory.loadFailed') }
  finally { if (!disposed && epoch === readEpoch) loading.value = false }
}
async function mutate(command: string, args: Record<string, unknown>) {
  if (busy.value) return false
  busy.value = true; error.value = ''
  try { accept(await invoke<UserMemory>(command, args)); return true }
  catch { if (!disposed) error.value = t('userMemory.saveFailed'); return false }
  finally { if (!disposed) busy.value = false }
}
async function toggle(event: Event) {
  const control = event.target as HTMLInputElement
  const enabled = control.checked
  control.checked = !!profile.value?.enabled
  await mutate('configure_user_memory', { enabled, autoLearn: enabled })
}
function edit(entry: UserMemoryEntry) { editing.value = { ...entry }; deleting.value = null }
async function save() {
  const entry = editing.value
  if (!entry?.content.trim()) return
  if (await mutate('edit_user_memory', { edit: { id: entry.id, category: entry.category, content: entry.content,
    knowledgeLevel: entry.category === 'knowledge' ? entry.knowledgeLevel : 'unspecified', locked: entry.locked,
    expectedUpdatedAt: entry.updatedAt } })) editing.value = null
}
async function remove() {
  if (!deleting.value) return
  const id = deleting.value === '*' ? null : deleting.value
  if (await mutate('delete_user_memory', { id })) {
    if (!id || editing.value?.id === id) editing.value = null
    deleting.value = null
  }
}
function date(value: string) { return new Date(value).toLocaleDateString(locale.value === 'zh' ? 'zh-CN' : 'en-US') }
onMounted(async () => {
  const unlisten = await listen('user-memory-changed', () => { void load() })
  if (disposed) { unlisten(); return }
  off = unlisten
  window.addEventListener('focus', load)
  await load()
})
onUnmounted(() => { disposed = true; readEpoch++; off?.(); window.removeEventListener('focus', load) })
</script>

<template>
  <div class="memory-settings">
    <div v-if="error" class="memory-error" role="alert">{{ error }} <button @click="load">{{ t('userMemory.retry') }}</button></div>
    <p v-if="loading">{{ t('userMemory.loading') }}</p>
    <template v-if="profile">
      <section class="memory-card memory-control">
        <label class="memory-toggle">
          <span><strong>{{ t('userMemory.enable') }}</strong><span class="memory-detail">{{ t('userMemory.enableHint') }}</span></span>
          <input type="checkbox" :checked="profile.enabled" :disabled="busy" @change="toggle">
        </label>
        <p class="memory-note">{{ t('userMemory.privacy') }}</p>
        <p class="memory-status" role="status">{{ !profile.enabled ? t('userMemory.disabled') : profile.updating ? t('userMemory.updating') : profile.lastError ? t('userMemory.learningFailed') : t('userMemory.automatic') }}</p>
      </section>

      <div class="memory-heading">
        <strong>{{ t('userMemory.profile') }} <span class="memory-count">{{ profile.entries.length }}</span></strong>
        <button v-if="profile.entries.length" class="memory-action" :disabled="busy" @click="deleting = '*'; editing = null">{{ t('userMemory.clear') }}</button>
      </div>
      <div v-if="deleting === '*'" class="memory-confirm" role="alert">
        <span>{{ t('userMemory.clearConfirm') }}</span>
        <div><button class="memory-action" :disabled="busy" @click="deleting = null">{{ t('userMemory.cancel') }}</button><button class="memory-action danger" :disabled="busy" @click="remove">{{ t('userMemory.confirmClear') }}</button></div>
      </div>
      <p v-if="!profile.entries.length" class="memory-empty">{{ t('userMemory.empty') }}</p>
      <section v-for="group in groups" :key="group.category" class="memory-group">
        <h3>{{ t('userMemory.categories.' + group.category) }}</h3>
        <article v-for="entry in group.entries" :key="entry.id" class="memory-card memory-entry">
          <form v-if="editing?.id === entry.id" @submit.prevent="save" @keydown.esc.prevent="editing = null">
            <label class="memory-field">{{ t('userMemory.content') }}<textarea v-model="editing.content" maxlength="300" rows="3" required /></label>
            <div class="memory-edit-options">
              <label class="memory-field">{{ t('userMemory.category') }}<select v-model="editing.category"><option v-for="category in categories" :key="category" :value="category">{{ t('userMemory.categories.' + category) }}</option></select></label>
              <label v-if="editing.category === 'knowledge'" class="memory-field">{{ t('userMemory.level') }}<select v-model="editing.knowledgeLevel"><option v-for="level in levels" :key="level" :value="level">{{ t('userMemory.levels.' + level) }}</option></select></label>
            </div>
            <label class="memory-lock"><input v-model="editing.locked" type="checkbox">{{ t('userMemory.lockHint') }}</label>
            <div class="memory-buttons"><button type="button" class="memory-action" :disabled="busy" @click="editing = null">{{ t('userMemory.cancel') }}</button><button type="submit" class="memory-save" :disabled="busy || !editing.content.trim()">{{ t('userMemory.save') }}</button></div>
          </form>
          <template v-else>
            <div class="memory-entry-heading"><span v-if="entry.category === 'knowledge' && entry.knowledgeLevel !== 'unspecified'" class="memory-level" :data-level="entry.knowledgeLevel">{{ t('userMemory.levels.' + entry.knowledgeLevel) }}</span><span v-if="entry.locked" class="memory-detail">{{ t('userMemory.locked') }}</span></div>
            <p class="memory-fact">{{ entry.content }}</p>
            <details v-if="entry.evidence" class="memory-evidence"><summary>{{ t('userMemory.evidence') }}</summary><blockquote>{{ entry.evidence }}</blockquote></details>
            <div class="memory-entry-footer"><span>{{ t(entry.source === 'manual' ? 'userMemory.corrected' : 'userMemory.learned') }} · {{ date(entry.updatedAt) }}</span><div><button class="memory-action" :disabled="busy" @click="edit(entry)">{{ t('userMemory.edit') }}</button><button class="memory-action" :disabled="busy" @click="deleting = entry.id">{{ t('userMemory.delete') }}</button></div></div>
            <div v-if="deleting === entry.id" class="memory-confirm" role="alert"><span>{{ t('userMemory.deleteConfirm') }}</span><div><button class="memory-action" :disabled="busy" @click="deleting = null">{{ t('userMemory.cancel') }}</button><button class="memory-action danger" :disabled="busy" @click="remove">{{ t('userMemory.delete') }}</button></div></div>
          </template>
        </article>
      </section>
    </template>
  </div>
</template>

<style scoped>
.memory-settings { display: flex; flex-direction: column; gap: 16px; color: var(--text-primary); font-size: 13px; }
.memory-card { background: var(--bg-secondary); border: 1px solid var(--border-default); border-radius: 12px; padding: 18px; }
.memory-toggle { display: flex; align-items: center; justify-content: space-between; gap: 24px; cursor: pointer; }
.memory-toggle strong { display: block; margin-bottom: 6px; font-size: 14px; }
input[type=checkbox] { accent-color: var(--accent); width: 16px; height: 16px; flex-shrink: 0; }
.memory-detail, .memory-note { font-size: 12px; color: var(--text-secondary); line-height: 1.6; }
.memory-note { margin: 14px 0 8px; }
.memory-status { font-size: 12px; color: var(--accent); margin: 0; line-height: 1.6; }
.memory-heading, .memory-entry-footer, .memory-entry-heading, .memory-buttons { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.memory-count { color: var(--text-secondary); font-weight: 400; margin-left: 6px; }
.memory-group { display: flex; flex-direction: column; gap: 8px; }
.memory-group h3 { font-size: 12px; font-weight: 500; color: var(--text-secondary); margin: 0 0 2px; }
.memory-entry { padding: 14px 16px 8px; }
.memory-entry-heading { justify-content: flex-start; }
.memory-level { border-radius: 5px; padding: 2px 7px; font-size: 11px; background: var(--bg-hover); color: var(--accent); }
.memory-fact { margin: 7px 0 10px; line-height: 1.65; overflow-wrap: anywhere; white-space: pre-wrap; }
.memory-evidence { color: var(--text-secondary); font-size: 11px; }
.memory-evidence summary { cursor: pointer; }
.memory-evidence blockquote { margin: 6px 0; padding-left: 10px; border-left: 2px solid var(--border-default); line-height: 1.5; overflow-wrap: anywhere; }
.memory-entry-footer { color: var(--text-tertiary); font-size: 11px; margin-top: 6px; flex-wrap: wrap; }
.memory-action { color: var(--text-secondary); padding: 6px 8px; border-radius: 5px; font-size: 12px; }
.memory-action:hover { color: var(--text-primary); background: var(--bg-hover); }
.memory-action.danger, .memory-error { color: var(--danger, #c85555); }
.memory-empty { color: var(--text-secondary); text-align: center; line-height: 1.8; padding: 25px 12px; }
.memory-confirm { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px; padding: 10px 0; font-size: 12px; }
.memory-field { display: flex; flex-direction: column; gap: 6px; color: var(--text-secondary); font-size: 12px; }
.memory-field textarea, .memory-field select { width: 100%; min-width: 0; box-sizing: border-box; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border-default); border-radius: 6px; padding: 8px 10px; font: inherit; }
.memory-field textarea { resize: vertical; line-height: 1.6; }
.memory-field :focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
.memory-edit-options { display: grid; grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); gap: 12px; margin-top: 12px; }
.memory-lock { display: flex; align-items: center; gap: 6px; margin-top: 12px; font-size: 12px; color: var(--text-secondary); }
.memory-buttons { justify-content: flex-end; margin: 10px 0 4px; }
.memory-save { border-radius: 6px; background: var(--accent); color: white; padding: 6px 14px; }
button:disabled { opacity: .5; cursor: default; }
</style>
