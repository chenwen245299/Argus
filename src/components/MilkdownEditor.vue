<script setup lang="ts">
import { Editor, rootCtx, defaultValueCtx, editorViewOptionsCtx } from '@milkdown/core'
import { commonmark } from '@milkdown/preset-commonmark'
import { Milkdown, useEditor } from '@milkdown/vue'
import { listener, listenerCtx } from '@milkdown/plugin-listener'
import { history } from '@milkdown/plugin-history'

const props = defineProps<{ initialContent: string }>()
const emit = defineEmits<{ change: [markdown: string] }>()

// This component MUST be rendered inside <MilkdownProvider>.
useEditor((root) =>
  Editor.make()
    .config((ctx) => {
      ctx.set(rootCtx, root)
      ctx.set(defaultValueCtx, props.initialContent)
      ctx.update(editorViewOptionsCtx, (prev) => ({
        ...prev,
        attributes: { class: 'argus-md-editor', spellcheck: 'false' },
      }))
      ctx.get(listenerCtx).markdownUpdated((_, markdown) => {
        emit('change', markdown)
      })
    })
    .use(commonmark)
    .use(listener)
    .use(history)
)
</script>

<template>
  <Milkdown />
</template>
