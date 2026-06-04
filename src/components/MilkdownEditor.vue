<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, nextTick } from 'vue'
import Vditor from 'vditor'
import 'vditor/dist/index.css'

const props = defineProps<{ initialContent: string }>()
const emit = defineEmits<{ change: [markdown: string] }>()

const containerEl = ref<HTMLDivElement | null>(null)
let vd: Vditor | null = null

onMounted(async () => {
  await nextTick()
  if (!containerEl.value) return

  const content = props.initialContent

  vd = new Vditor(containerEl.value, {
    mode: 'ir',
    cdn: '/vditor',
    value: content,
    minHeight: 200,
    toolbarConfig: { hide: true },
    preview: {
      math: {
        engine: 'KaTeX',
        inlineDigit: true,
      },
    },
    counter: { enable: false },
    resize: { enable: false },
    cache: { enable: false },
    after() {
      // Vditor normally consumes `value` during init. This catches the rare
      // first-mount case where async IR rendering leaves the editor empty.
      requestAnimationFrame(() => {
        if (content && vd?.getValue().trim() === '') {
          vd.setValue(content, true)
        }
      })
    },
    input(val) {
      emit('change', val)
    },
  })
})

onBeforeUnmount(() => {
  vd?.destroy()
  vd = null
})
</script>

<template>
  <div ref="containerEl" class="vditor-host" />
</template>

<style scoped>
/* Absolutely fill editor-wrap so Vditor can never escape and cover the toolbar */
.vditor-host {
  position: absolute;
  inset: 0;
  overflow-y: auto;
}
</style>
