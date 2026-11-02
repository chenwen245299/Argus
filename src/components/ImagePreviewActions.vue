<script setup lang="ts">
import { ref, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { copyImageToClipboard } from '../utils/copyImage'

const props = defineProps<{ src: string }>()
const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()
const state = ref<'idle' | 'copying' | 'copied' | 'failed'>('idle')
let revision = 0
watch(() => props.src, () => { revision++; state.value = 'idle' })

async function copy() {
  if (state.value === 'copying') return
  const current = ++revision
  state.value = 'copying'
  try {
    await copyImageToClipboard(props.src)
    if (current === revision) state.value = 'copied'
  } catch {
    if (current === revision) state.value = 'failed'
  }
}
</script>

<template>
  <div class="image-preview-actions" @click.stop @keydown.esc.stop.prevent="emit('close')">
    <span v-if="state === 'copied' || state === 'failed'" class="image-copy-status" role="status">
      {{ t(state === 'copied' ? 'chat.imageCopied' : 'chat.imageCopyFailed') }}
    </span>
    <button type="button" class="image-copy" :disabled="state === 'copying'"
      :title="t('chat.imageCopy')" :aria-label="t('chat.imageCopy')" @click="copy">
      <Icon :icon="state === 'copied' ? 'fluent:checkmark-24-regular' : 'fluent:copy-24-regular'" width="16" height="16" />
      {{ t(state === 'copying' ? 'chat.imageCopying' : 'chat.imageCopy') }}
    </button>
    <button type="button" class="image-preview-close" :title="t('chat.imageClose')"
      :aria-label="t('chat.imageClose')" @click="emit('close')">
      <Icon icon="fluent:dismiss-24-regular" width="18" height="18" />
    </button>
  </div>
</template>

<style scoped>
.image-preview-actions {
  position: absolute;
  top: 10px;
  right: 10px;
  left: 10px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  pointer-events: none;
}
.image-preview-actions button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  gap: 6px;
  height: 32px;
  padding: 0 10px;
  border: 1px solid rgba(255, 255, 255, .2);
  border-radius: 8px;
  background: rgba(0, 0, 0, .65);
  color: #fff;
  font-size: 12px;
  cursor: pointer;
  pointer-events: auto;
}
.image-preview-actions button:hover { background: rgba(0, 0, 0, .8); }
.image-preview-actions button:focus-visible { outline: 2px solid #fff; outline-offset: 2px; }
.image-preview-actions button:disabled { opacity: .65; cursor: wait; }
.image-preview-actions .image-preview-close { width: 32px; padding: 0; border-radius: 50%; }
.image-copy-status {
  min-width: 0;
  padding: 5px 8px;
  border-radius: 6px;
  background: rgba(0, 0, 0, .65);
  color: #fff;
  font-size: 12px;
  line-height: 1.4;
}
</style>
