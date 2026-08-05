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
// With a computed, a finished answer parses once and costs nothing afterwards;
// only the answer actually streaming re-parses.
const props = defineProps<{ content: string; streaming?: boolean }>()

const segments = computed(() => getSegments(props.content, props.streaming))
</script>

<template>
  <template v-for="(seg, i) in segments" :key="i">
    <div v-if="seg.type === 'md'" v-html="seg.html" />
    <MermaidBlock v-else :src="seg.src" />
  </template>
</template>
