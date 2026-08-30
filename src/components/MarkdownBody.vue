<script setup lang="ts">
import { computed } from 'vue'
import { getSegments } from '../utils/renderMarkdown'
import MermaidBlock from './MermaidBlock.vue'

// Rendered markdown, parsed inside a `computed` — that caching is the entire
// reason this component exists.
//
// Calling getSegments()/renderMarkdown() straight from a chat template looks
// harmless but re-parses on EVERY re-render of the whole message list, because
// template expressions are not memoized. While one answer streams the list
// re-renders ~11x/second, so every previously finished answer was being run
// through marked + KaTeX + highlight.js + DOMPurify again each time: ~9.5ms per
// answer, i.e. ~1.7s of CPU per second of streaming at 20 turns. The main thread
// saturates, the webview stops painting (white gaps while scrolling) and every
// other view sharing it — the PDF reader included — freezes with it.
//
// With a computed, a finished answer parses once and costs nothing afterwards.
//
// While an answer is STILL streaming we go further and skip the parse entirely:
// re-parsing a single growing answer is O(n²) on its own (each refresh re-parses
// all the text so far), so a long answer — especially with a long context behind
// it — freezes the UI even with throttling. So during streaming we show the raw
// text as-is (whitespace preserved) and only render the markdown/mermaid once,
// the moment streaming ends. `streaming` flips false → the computed runs its
// single real parse.
const props = defineProps<{ content: string; streaming?: boolean }>()

const segments = computed(() => (props.streaming ? [] : getSegments(props.content, false)))
</script>

<template>
  <div v-if="streaming" class="md-streaming-plain">{{ content }}</div>
  <template v-else v-for="(seg, i) in segments" :key="i">
    <div v-if="seg.type === 'md'" v-html="seg.html" />
    <MermaidBlock v-else :src="seg.src" />
  </template>
</template>

<style scoped>
/* Raw, unparsed answer shown while it is still streaming. Preserve the model's
   own newlines and spacing so it reads naturally before the final markdown
   render swaps in. */
.md-streaming-plain {
  white-space: pre-wrap;
  word-break: break-word;
}
</style>
