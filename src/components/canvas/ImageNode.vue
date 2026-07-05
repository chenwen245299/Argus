<script setup lang="ts">
import { computed } from 'vue'
import { Handle, Position } from '@vue-flow/core'
import type { NodeProps } from '@vue-flow/core'

export interface ImageNodeData {
  src?: string
  alt?: string
  width?: number
  height?: number
  cornerRadius?: number
  rotation?: number
  opacity?: number
}

const props = defineProps<NodeProps<ImageNodeData>>()

const nodeStyle = computed(() => {
  const style: Record<string, string> = {
    width: `${props.data.width ?? 260}px`,
    height: `${props.data.height ?? 180}px`,
    borderRadius: `${props.data.cornerRadius ?? 8}px`,
    opacity: String(props.data.opacity ?? 1),
  }
  if (props.data.rotation) style.transform = `rotate(${props.data.rotation}deg)`
  return style
})
</script>

<template>
  <div class="image-node" :class="{ 'image-node--selected': selected }" :style="nodeStyle">
    <img v-if="data.src" class="image-node-img" :src="data.src" :alt="data.alt || 'Pasted image'" draggable="false" />
    <div v-else class="image-node-empty">图片</div>

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
.image-node {
  position: relative;
  overflow: hidden;
  background: var(--bg-primary);
  border: 1.5px solid var(--border-default);
  box-shadow: var(--shadow-sm);
  cursor: default;
  user-select: none;
}

.image-node--selected,
.image-node:hover {
  border-color: var(--accent);
}

.image-node-img {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
  background: var(--bg-secondary);
  pointer-events: none;
}

.image-node-empty {
  display: grid;
  width: 100%;
  height: 100%;
  place-items: center;
  color: var(--text-tertiary);
  font-size: 13px;
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
.image-node:hover .node-handle,
.image-node--selected .node-handle {
  opacity: 1;
}
</style>
