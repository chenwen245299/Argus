<script setup lang="ts">
import { computed } from 'vue'
import { Handle, Position } from '@vue-flow/core'
import type { NodeProps } from '@vue-flow/core'

export interface TextNodeData {
  content?: string
  color?: string
  fontSize?: number
  bold?: boolean
  italic?: boolean
}

const props = defineProps<NodeProps<TextNodeData>>()

const textStyle = computed(() => ({
  color: props.data.color ?? 'var(--text-primary)',
  fontSize: (props.data.fontSize ?? 14) + 'px',
  fontWeight: props.data.bold ? '700' : '400',
  fontStyle: props.data.italic ? 'italic' : 'normal',
}))
</script>

<template>
  <div class="text-node">
    <div class="text-content" :style="textStyle">{{ data.content || '双击编辑文字' }}</div>

    <Handle id="src-top"    :position="Position.Top"    type="source" class="node-handle" />
    <Handle id="src-right"  :position="Position.Right"  type="source" class="node-handle" />
    <Handle id="src-bottom" :position="Position.Bottom" type="source" class="node-handle" />
    <Handle id="src-left"   :position="Position.Left"   type="source" class="node-handle" />
    <Handle id="tgt-top"    :position="Position.Top"    type="target" class="node-handle" />
    <Handle id="tgt-right"  :position="Position.Right"  type="target" class="node-handle" />
    <Handle id="tgt-bottom" :position="Position.Bottom" type="target" class="node-handle" />
    <Handle id="tgt-left"   :position="Position.Left"   type="target" class="node-handle" />
  </div>
</template>

<style scoped>
.text-node {
  position: relative;
  padding: 6px 10px;
  min-width: 60px;
  max-width: 320px;
  cursor: default;
  user-select: none;
  border: 1.5px solid transparent;
  border-radius: 5px;
  transition: border-color 0.12s;
}
.text-node:hover {
  border-color: var(--border-default);
}

.text-content {
  line-height: 1.5;
  white-space: pre-wrap;
  word-break: break-word;
}

.node-handle {
  width: 10px;
  height: 10px;
  background: var(--accent);
  border: 2px solid var(--bg-primary);
  border-radius: 50%;
  opacity: 0;
  transition: opacity 0.15s;
}
.text-node:hover .node-handle { opacity: 1; }
</style>
