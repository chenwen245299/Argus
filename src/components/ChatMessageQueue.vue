<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import type { QueuedChatMessage } from '../utils/chatSendQueue'
import type { Attachment } from '../utils/attachments'

const props = defineProps<{ items: QueuedChatMessage[]; paused?: boolean }>()
const emit = defineEmits<{
  edit: [id: string, text: string, files: Attachment[]]
  hold: [id: string, on: boolean]
  steer: [id: string]
  remove: [id: string]
  resume: []
}>()
const { t } = useI18n()
const editing = ref<string | null>(null)
const text = ref('')
const files = ref<Attachment[]>([])
const rootEl = ref<HTMLElement | null>(null)
function begin(item: QueuedChatMessage) {
  finish(false)
  editing.value = item.id
  text.value = item.text
  files.value = [...item.files]
  emit('hold', item.id, true)
  void nextTick(() => {
    const el = rootEl.value?.querySelector<HTMLTextAreaElement>('.queue-edit')
    el?.focus()
    el?.setSelectionRange(el.value.length, el.value.length)
  })
}
function finish(save: boolean) {
  const id = editing.value
  if (!id) return
  if (save && !text.value.trim() && !files.value.length) return
  if (save) emit('edit', id, text.value, files.value)
  editing.value = null
  emit('hold', id, false)
}
function keydown(event: KeyboardEvent) {
  if (event.isComposing || event.keyCode === 229) return
  if (event.key === 'Escape') { event.preventDefault(); finish(false) }
  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); finish(true) }
}
watch(() => props.items.map(item => item.id).join('|'), () => {
  if (editing.value && !props.items.some(item => item.id === editing.value)) finish(false)
})
</script>

<template>
  <section v-if="items.length" ref="rootEl" class="chat-message-queue" :aria-label="t('chat.queueTitle')">
    <div class="queue-heading">
      <span>{{ t(paused ? 'chat.queuePaused' : 'chat.queueTitle') }} · {{ items.length }}</span>
      <button v-if="paused" type="button" @click="emit('resume')">{{ t('chat.queueResume') }}</button>
    </div>
    <div class="queue-list">
      <div v-for="(item, index) in items" :key="item.id" class="queue-item">
        <span class="queue-number">{{ index + 1 }}</span>
        <div class="queue-body">
          <template v-if="editing === item.id">
            <textarea v-model="text" class="queue-edit" :aria-label="t('chat.queueEdit')" rows="2" @keydown="keydown" />
            <div v-if="files.length" class="queue-files">
              <button v-for="file in files" :key="file.id" type="button" :title="t('chat.queueRemoveAttachment')" @click="files = files.filter(f => f.id !== file.id)">{{ file.name }} ×</button>
            </div>
            <div class="queue-actions">
              <button type="button" :disabled="!text.trim() && !files.length" @click="finish(true)">{{ t('chat.queueSave') }}</button>
              <button type="button" @click="finish(false)">{{ t('chat.queueCancel') }}</button>
            </div>
          </template>
          <template v-else>
            <p class="queue-text">{{ item.text || t('chat.queueAttachmentsOnly') }}</p>
            <div v-if="item.files.length" class="queue-file-names">{{ item.files.map(file => file.name).join(' · ') }}</div>
            <div class="queue-actions">
              <button type="button" @click="begin(item)"><Icon icon="fluent:edit-16-regular" />{{ t('chat.queueEdit') }}</button>
              <button type="button" class="queue-steer" :title="t('chat.steerHint')" @click="emit('steer', item.id)"><Icon icon="fluent:arrow-turn-up-right-20-regular" />{{ t('chat.queueSteer') }}</button>
              <button type="button" @click="emit('remove', item.id)"><Icon icon="fluent:delete-16-regular" />{{ t('chat.queueDelete') }}</button>
            </div>
          </template>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.chat-message-queue { flex-shrink: 0; border: 1px solid var(--border-default); border-radius: 9px; background: var(--bg-secondary); margin-bottom: 8px; font-size: 12px; }
.queue-heading { display: flex; justify-content: space-between; align-items: center; padding: 7px 10px; color: var(--text-secondary); border-bottom: 1px solid var(--border-subtle); }
.queue-list { max-height: 210px; overflow-y: auto; }
.queue-item { display: flex; gap: 8px; padding: 8px 10px; }
.queue-item + .queue-item { border-top: 1px solid var(--border-subtle); }
.queue-number { color: var(--text-tertiary); padding-top: 2px; }
.queue-body { flex: 1; min-width: 0; }
.queue-text { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
.queue-file-names { color: var(--text-secondary); font-size: 11px; margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.queue-actions, .queue-files { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 5px; }
button { display: inline-flex; align-items: center; gap: 3px; font: inherit; border: 0; border-radius: 4px; padding: 3px 5px; background: transparent; color: var(--text-secondary); cursor: pointer; }
button:hover { color: var(--text-primary); background: var(--bg-hover); }
button:disabled { opacity: .4; cursor: default; }
.queue-steer { color: var(--accent); }
.queue-edit { display: block; width: 100%; box-sizing: border-box; resize: vertical; min-height: 52px; max-height: 160px; padding: 6px; border: 1px solid var(--border-default); border-radius: 5px; background: var(--bg-primary); color: var(--text-primary); font: inherit; }
</style>
