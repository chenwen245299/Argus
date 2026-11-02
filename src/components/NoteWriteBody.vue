<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import MarkdownBody from './MarkdownBody.vue'
import { toDisplayMarkdown } from '../utils/noteAssets'

const props = defineProps<{
  slug: string
  content: string
  images?: { path: string; dataUrl: string }[]
}>()
const display = ref('')
let revision = 0
let urls: string[] = []
function release() {
  urls.forEach(url => URL.revokeObjectURL(url))
  urls = []
}

watch(() => [props.slug, props.content, props.images] as const, async () => {
  const current = ++revision
  release()
  // Show text immediately; only our captured pixels and existing note assets
  // are resolved. Blob URLs keep multi-MB data out of the Markdown parser.
  let markdown = props.content
  for (const image of props.images ?? []) {
    const prefix = 'data:image/png;base64,'
    if (!/^assets\/ai-[a-f0-9-]+\.png$/.test(image.path) || !image.dataUrl.startsWith(prefix)) continue
    const bytes = Uint8Array.from(atob(image.dataUrl.slice(prefix.length)), c => c.charCodeAt(0))
    const url = URL.createObjectURL(new Blob([bytes], { type: 'image/png' }))
    urls.push(url)
    markdown = markdown.replaceAll(`](${image.path})`, `](${url})`)
  }
  display.value = markdown
  const resolved = await toDisplayMarkdown(props.slug, markdown)
  if (current === revision) display.value = resolved
}, { immediate: true })

onBeforeUnmount(() => { revision++; release() })
</script>

<template>
  <MarkdownBody :content="display" />
</template>
