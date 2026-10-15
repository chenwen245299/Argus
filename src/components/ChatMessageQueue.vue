<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import type { QueuedChatMessage } from '../utils/chatSendQueue'

const props = defineProps<{ items: QueuedChatMessage[]; paused?: boolean; editingId?: string | null }>()
const emit = defineEmits<{
  edit: [id: string]
  steer: [id: string]
  remove: [id: string]
  resume: []
}>()
const { t } = useI18n()
const menuId = ref<string | null>(null)
const menuEl = ref<HTMLElement | null>(null)
const menuPosition = ref({ left: '0px', top: '0px', visibility: 'hidden' as 'hidden' | 'visible' })
const menuItem = computed(() => props.items.find(item => item.id === menuId.value))
let menuTrigger: HTMLButtonElement | null = null

function preview(item: QueuedChatMessage) {
  return item.text || item.files.map(file => file.name).join(' · ') || t('chat.queueAttachmentsOnly')
}
function previewTitle(item: QueuedChatMessage) {
  return [item.text, item.files.map(file => file.name).join(' · ')].filter(Boolean).join('\n')
}
function closeMenu(restoreFocus = false) {
  menuId.value = null
  if (restoreFocus) menuTrigger?.focus()
}
function positionMenu() {
  if (!menuTrigger || !menuEl.value) return
  const anchor = menuTrigger.getBoundingClientRect(), menu = menuEl.value.getBoundingClientRect()
  const below = anchor.bottom + 5
  menuPosition.value = {
    left: `${Math.max(8, Math.min(anchor.right - menu.width, innerWidth - menu.width - 8))}px`,
    top: `${Math.max(8, below + menu.height <= innerHeight - 8 ? below : anchor.top - menu.height - 5)}px`,
    visibility: 'visible',
  }
}
async function openMenu(item: QueuedChatMessage, event: MouseEvent) {
  if (menuId.value === item.id) { closeMenu(true); return }
  menuTrigger = event.currentTarget as HTMLButtonElement
  menuId.value = item.id
  menuPosition.value.visibility = 'hidden'
  await nextTick()
  positionMenu()
  await nextTick()
  if (menuId.value === item.id) menuEl.value?.querySelector<HTMLButtonElement>('button')?.focus()
}
function editFromMenu() {
  const item = menuItem.value
  closeMenu()
  if (item) emit('edit', item.id)
}
function removeFromMenu() {
  const item = menuItem.value
  closeMenu(true)
  if (item) emit('remove', item.id)
}
function dismissMenu(event: MouseEvent) {
  if (!menuEl.value?.contains(event.target as Node) && !menuTrigger?.contains(event.target as Node)) closeMenu()
}
function menuKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); closeMenu(true) }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    const buttons = [...(menuEl.value?.querySelectorAll<HTMLButtonElement>('button') ?? [])]
    const index = buttons.findIndex(button => button === document.activeElement)
    buttons[(index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus()
  }
  if (event.key === 'Tab') closeMenu()
}
onMounted(() => {
  document.addEventListener('mousedown', dismissMenu)
  window.addEventListener('resize', positionMenu)
})
onUnmounted(() => {
  document.removeEventListener('mousedown', dismissMenu)
  window.removeEventListener('resize', positionMenu)
})
watch(() => props.items.map(item => item.id).join('|'), () => {
  if (menuId.value && !menuItem.value) closeMenu()
})
</script>

<template>
  <section v-if="items.length" class="chat-message-queue" :aria-label="t('chat.queueTitle')">
    <div v-if="paused" class="queue-heading">
      <span>{{ t('chat.queuePaused') }}</span>
      <button type="button" @click="emit('resume')">{{ t('chat.queueResume') }}</button>
    </div>
    <div class="queue-list" @scroll="closeMenu()">
      <div v-for="item in items" :key="item.id" class="queue-item" :class="{ editing: editingId === item.id }">
        <Icon class="queue-mark" :icon="editingId === item.id ? 'fluent:edit-16-regular' : 'fluent:arrow-turn-down-right-20-regular'" width="15" height="15" aria-hidden="true" />
        <div class="queue-body">
          <p class="queue-text" :title="previewTitle(item)">{{ preview(item) }}</p>
        </div>
        <div class="queue-controls">
          <span v-if="item.text && item.files.length" class="queue-file-count" :title="item.files.map(file => file.name).join(' · ')">
            <Icon icon="fluent:attach-16-regular" width="12" height="12" />{{ item.files.length }}
          </span>
          <button type="button" class="queue-steer" :title="t('chat.steerHint')" @click="emit('steer', item.id)">
            <Icon icon="fluent:arrow-hook-down-right-20-regular" width="15" height="15" />{{ t('chat.queueSteer') }}
          </button>
          <button type="button" class="queue-icon-button" :title="t('chat.queueDelete')" :aria-label="t('chat.queueDelete')" @click="emit('remove', item.id)">
            <Icon icon="fluent:delete-16-regular" width="15" height="15" />
          </button>
          <button type="button" class="queue-icon-button queue-more" :class="{ active: menuId === item.id }"
            :title="t('chat.queueMore')" :aria-label="t('chat.queueMore')" aria-haspopup="menu" :aria-expanded="menuId === item.id" @click="openMenu(item, $event)">
            <Icon icon="fluent:more-horizontal-20-regular" width="16" height="16" />
          </button>
        </div>
      </div>
    </div>
  </section>
  <Teleport to="body">
    <div v-if="menuItem" ref="menuEl" class="queue-menu" :style="menuPosition" role="menu" :aria-label="t('chat.queueMore')" @keydown="menuKeydown">
      <button type="button" role="menuitem" @click="editFromMenu"><Icon icon="fluent:edit-16-regular" width="16" height="16" />{{ t('chat.queueEditMessage') }}</button>
      <button type="button" role="menuitem" @click="removeFromMenu"><Icon icon="fluent:arrow-turn-down-right-20-regular" width="16" height="16" />{{ t('chat.queueRemoveQueued') }}</button>
    </div>
  </Teleport>
</template>

<style scoped>
.chat-message-queue { position: relative; z-index: 0; flex-shrink: 0; border: 1px solid var(--border-subtle); border-bottom: 0; border-radius: 18px 18px 0 0; background: color-mix(in srgb, var(--bg-secondary) 80%, var(--bg-primary)); margin: 0 14px -16px; padding-bottom: 16px; font-size: 12px; overflow: hidden; }
.queue-heading { display: flex; justify-content: space-between; align-items: center; padding: 4px 10px; color: var(--text-tertiary); border-bottom: 1px solid var(--border-subtle); font-size: 11px; }
.queue-list { max-height: min(152px, 25vh); overflow-y: auto; overflow-x: hidden; overscroll-behavior: contain; }
.queue-item { display: flex; align-items: center; gap: 8px; min-height: 38px; box-sizing: border-box; padding: 5px 8px 5px 12px; }
.queue-item + .queue-item { border-top: 1px solid var(--border-subtle); }
.queue-mark { flex-shrink: 0; color: var(--text-tertiary); }
.queue-body { flex: 1; min-width: 0; }
.queue-text { margin: 0; color: var(--text-primary); font-size: 13px; font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; line-height: 20px; }
.queue-controls { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.queue-file-count { display: inline-flex; align-items: center; gap: 2px; color: var(--text-tertiary); font-size: 10px; }
button { display: inline-flex; align-items: center; gap: 5px; font: inherit; border: 0; border-radius: 7px; padding: 4px 6px; background: transparent; color: var(--text-secondary); cursor: pointer; }
button:hover { color: var(--text-primary); background: var(--bg-hover); }
button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
button:disabled { opacity: .4; cursor: default; }
.queue-steer { color: var(--text-tertiary); white-space: nowrap; padding: 4px; }
.queue-icon-button { width: 24px; height: 24px; justify-content: center; padding: 0; background: transparent; }
.queue-icon-button:hover, .queue-icon-button.active { background: transparent; color: var(--text-primary); }
.queue-item.editing .queue-mark { color: var(--accent); }
.queue-menu { position: fixed; z-index: 1200; min-width: 190px; box-sizing: border-box; padding: 7px; border: 1px solid var(--border-default); border-radius: 18px; background: var(--bg-secondary); box-shadow: var(--shadow-lg); color: var(--text-primary); font-size: 13px; }
.queue-menu button { width: 100%; gap: 9px; padding: 10px 11px; border-radius: 11px; color: var(--text-primary); }
</style>
