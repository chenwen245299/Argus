<script setup lang="ts">
import { computed } from 'vue'
import ChatPageImage from './ChatPageImage.vue'
import { SEND_PAPER_IMAGE_TOOL, type AgentStepImage } from '../types/chatImages'

const props = defineProps<{
  conversationId: string | null
  steps?: { tool: string; ok?: boolean; images?: AgentStepImage[] }[]
}>()
defineEmits<{ (e: 'open', src: string): void }>()

// The tool trail already owns and persists the image references. Deriving this
// gallery keeps answer variants and reloaded conversations in sync without a
// second copy of the image data in the chat JSON.
const images = computed(() => props.steps?.flatMap(step =>
  step.tool === SEND_PAPER_IMAGE_TOOL && step.ok === true ? step.images ?? [] : [],
) ?? [])
</script>

<template>
  <div v-if="images.length" class="chat-sent-images">
    <ChatPageImage
      v-for="(image, index) in images"
      :key="image.file || `${image.slug}-${image.page}-${index}`"
      :conversation-id="conversationId"
      :image="image"
      presentation="message"
      @open="$emit('open', $event)"
    />
  </div>
</template>

<style scoped>
.chat-sent-images {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin: 12px 0;
  min-width: 0;
}
</style>
