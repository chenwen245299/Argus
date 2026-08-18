<template>
  <button
    v-if="src"
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
import { ref, onMounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { Icon } from '@iconify/vue'

interface AgentStepImage {
  slug: string
  page: number
  /** Filename in the conversation's image folder. */
  file?: string
  /** Live data URL from the render event; absent after a reload. */
  dataUrl?: string
}

const props = defineProps<{
  conversationId: string | null
  image: AgentStepImage
}>()
defineEmits<{ (e: 'open', src: string): void }>()

const src = ref<string | null>(props.image.dataUrl ?? null)

onMounted(async () => {
  // A live render already carries the data URL; a reloaded conversation only has
  // the file name, so fetch the saved PNG from the conversation's image folder.
  if (src.value || !props.image.file || !props.conversationId) return
  try {
    src.value = await invoke<string>('read_chat_image', {
      conversationId: props.conversationId,
      file: props.image.file,
    })
  } catch {
    // A missing or deleted image just shows nothing rather than breaking the chat.
  }
})
</script>

<style scoped>
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
