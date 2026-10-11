<script setup lang="ts">
import { computed } from 'vue'
import MarkdownBody from './MarkdownBody.vue'
import ChatPageImage from './ChatPageImage.vue'
import { chatImageContent, type ImageStep } from '../utils/chatImageContent'

const props = defineProps<{
  content: string
  streaming?: boolean
  conversationId: string | null
  steps?: ImageStep[]
}>()
defineEmits<{ (e: 'open', src: string): void }>()

// Splitting references is cheap; MarkdownBody still skips the expensive markdown
// parse while streaming and caches each finished text block afterwards.
const body = computed(() => chatImageContent(props.content, props.steps))
</script>

<template>
  <template v-for="(part, index) in body.parts" :key="index">
    <MarkdownBody v-if="part.type === 'text'" :content="part.content" :streaming="streaming" />
    <div v-else class="chat-answer-image">
      <ChatPageImage :conversation-id="conversationId" :image="part.image" presentation="message" @open="$emit('open', $event)" />
    </div>
  </template>
  <!-- Old answers have no inline references. Keep their sent images visible,
       and also retain images omitted from a newer answer or interrupted turn. -->
  <div v-if="!streaming && body.remaining.length" class="chat-answer-images">
    <ChatPageImage
      v-for="(image, index) in body.remaining"
      :key="image.file || `${image.slug}-${image.page}-${index}`"
      :conversation-id="conversationId"
      :image="image"
      presentation="message"
      @open="$emit('open', $event)"
    />
  </div>
</template>

<style scoped>
.chat-answer-image, .chat-answer-images { margin: 12px 0; min-width: 0; }
.chat-answer-images { display: flex; flex-direction: column; gap: 12px; }
</style>
