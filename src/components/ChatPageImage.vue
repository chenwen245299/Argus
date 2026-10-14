<template>
  <figure v-if="presentation === 'message'" class="chat-image-message">
    <button
      v-if="src"
      type="button"
      class="chat-image-open"
      :title="t('chat.imageEnlarge')"
      @click="$emit('open', src)"
    >
      <img
        :src="src"
        :alt="image.caption || t('chat.imagePage', { page: image.page })"
        :width="image.width"
        :height="image.height"
      />
    </button>
    <span v-else class="chat-image-placeholder" role="status">
      <Icon width="18" height="18" icon="fluent:image-24-regular" />
      {{ t(loadFailed ? 'chat.imageUnavailable' : 'chat.imageLoading') }}
    </span>
    <figcaption>
      <span v-if="image.caption" class="chat-image-caption">{{ image.caption }}</span>
      <span class="chat-image-source" :title="image.slug">
        {{ t('chat.imagePage', { page: image.page }) }}<template v-if="image.crop"> · {{ t('chat.imageCropped') }}</template>
      </span>
    </figcaption>
  </figure>
  <button
    v-else-if="src"
    class="chat-page-image"
    :title="`第 ${image.page} 页（点击查看大图）`"
    @click="$emit('open', src)"
  >
    <img :src="src" class="chat-page-image-thumb" alt="" />
    <span class="chat-page-image-label">p{{ image.page }}</span>
  </button>
  <span v-else class="chat-page-image chat-page-image-loading" :title="`第 ${image.page} 页`">
    <Icon width="14" height="14" icon="fluent:image-24-regular" />
  </span>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import type { AgentStepImage } from '../types/chatImages'

const props = withDefaults(defineProps<{
  conversationId: string | null
  image: AgentStepImage
  presentation?: 'thumbnail' | 'message'
}>(), { presentation: 'thumbnail' })
defineEmits<{ (e: 'open', src: string): void }>()

const { t } = useI18n()
const src = ref<string | null>(props.image.dataUrl ?? null)
const loadFailed = ref(false)

// Sync/disk snapshots replace image objects even when the saved PNG is unchanged.
// Watch the source values separately so those replacements do not clear the <img>
// or cancel an outstanding read just because a fresh array was returned.
watch([
  () => props.conversationId,
  () => props.image.file,
  () => props.image.dataUrl,
], async ([conversationId, file, dataUrl], [previousConversationId, previousFile], onCleanup) => {
  let current = true
  onCleanup(() => { current = false })
  loadFailed.value = false
  if (dataUrl) {
    src.value = dataUrl
    return
  }
  // Persistence strips live data URLs. The immutable file still identifies the
  // pixels already on screen, so keep them through that transition.
  if (file && file === previousFile && conversationId === previousConversationId && src.value) return
  src.value = null
  // A live render already carries the data URL; a reloaded conversation only has
  // the file name, so fetch the saved PNG from the conversation's image folder.
  if (!file || !conversationId) {
    loadFailed.value = true
    return
  }
  try {
    const url = await invoke<string>('read_chat_image', {
      conversationId,
      file,
    })
    if (current) src.value = url
  } catch {
    if (current) loadFailed.value = true
  }
}, { immediate: true })
</script>

<style scoped>
.chat-image-message {
  margin: 0;
  min-width: 0;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md, 8px);
  overflow: hidden;
  background: var(--bg-secondary);
}
.chat-image-open {
  display: block;
  width: 100%;
  padding: 0;
  cursor: zoom-in;
}
.chat-image-open img {
  display: block;
  width: 100%;
  height: auto;
  max-height: 480px;
  object-fit: contain;
  background: var(--bg-primary);
}
.chat-image-message figcaption {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 8px 10px;
  line-height: 1.5;
}
.chat-image-caption {
  font-size: 12px;
  color: var(--text-primary);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.chat-image-source {
  font-size: 11px;
  color: var(--text-secondary);
}
.chat-image-placeholder {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  min-height: 100px;
  color: var(--text-secondary);
  font-size: 12px;
}
.chat-page-image {
  position: relative;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 68px;
  padding: 0;
  overflow: hidden;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md, 8px);
  background: var(--bg-secondary);
  cursor: pointer;
}
.chat-page-image-thumb {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.chat-page-image-label {
  position: absolute;
  right: 0;
  bottom: 0;
  padding: 1px 3px;
  font-size: 9px;
  line-height: 1.2;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border-top-left-radius: 4px;
}
.chat-page-image-loading {
  cursor: default;
  color: var(--text-secondary);
}
</style>
