<script setup lang="ts">
import { computed } from 'vue'
import { Handle, Position } from '@vue-flow/core'
import type { NodeProps } from '@vue-flow/core'

export interface ShapeNodeData {
  content?: string
  color?: string
  fillColor?: string
  width?: number
  height?: number
}

const props = defineProps<NodeProps<ShapeNodeData>>()

const borderColor = computed(() => props.data.color ?? 'var(--border-default)')
const fillColor = computed(() => props.data.fillColor ?? 'transparent')
const shapeStyle = computed(() => ({
  width: `${props.data.width ?? 160}px`,
  height: `${props.data.height ?? 100}px`,
  borderColor: borderColor.value,
  background: fillColor.value,
}))
</script>

<template>
  <div
    class="shape-node"
    :style="shapeStyle"
  >
    <div v-if="data.content" class="shape-label">{{ data.content }}</div>

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
.shape-node {
  position: relative;
  border: 2px solid var(--border-default);
  border-radius: 6px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: default;
  user-select: none;
}

.shape-label {
  font-size: 13px;
  color: var(--text-primary);
  text-align: center;
  padding: 4px 8px;
  word-break: break-word;
  pointer-events: none;
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
.shape-node:hover .node-handle { opacity: 1; }
</style>
