<script setup lang="ts">
import { computed } from 'vue'
import type { NodeProps } from '@vue-flow/core'

export interface LineNodeData {
  lineKind?: 'line' | 'arrow'
  color?: string
  strokeWidth?: number
  width?: number
  height?: number
  x1?: number
  y1?: number
  x2?: number
  y2?: number
}

const props = defineProps<NodeProps<LineNodeData>>()

const w = computed(() => Math.max(1, props.data.width ?? 1))
const h = computed(() => Math.max(1, props.data.height ?? 1))
const stroke = computed(() => props.data.color ?? '#1a1a1a')
const strokeWidth = computed(() => props.data.strokeWidth ?? 2)
const isArrow = computed(() => (props.data.lineKind ?? 'arrow') === 'arrow')
// Unique marker id per node so arrowheads pick up each line's own color.
const markerId = computed(() => `arrow-${props.id}`)
</script>

<template>
  <div class="line-node" :style="{ width: `${w}px`, height: `${h}px` }">
    <svg
      class="line-svg"
      :width="w"
      :height="h"
      :viewBox="`0 0 ${w} ${h}`"
      preserveAspectRatio="none"
      overflow="visible"
    >
      <defs v-if="isArrow">
        <marker
          :id="markerId"
          markerWidth="10"
          markerHeight="10"
          refX="8"
          refY="3"
          orient="auto"
          markerUnits="strokeWidth"
        >
          <path d="M0,0 L8,3 L0,6 z" :fill="stroke" />
        </marker>
      </defs>
      <!-- Fat transparent hit line so thin lines are easy to grab/select -->
      <line
        :x1="data.x1 ?? 0" :y1="data.y1 ?? 0"
        :x2="data.x2 ?? w" :y2="data.y2 ?? h"
        stroke="transparent"
        :stroke-width="Math.max(12, strokeWidth + 8)"
        stroke-linecap="round"
      />
      <line
        :x1="data.x1 ?? 0" :y1="data.y1 ?? 0"
        :x2="data.x2 ?? w" :y2="data.y2 ?? h"
        :stroke="stroke"
        :stroke-width="strokeWidth"
        stroke-linecap="round"
        :marker-end="isArrow ? `url(#${markerId})` : undefined"
      />
    </svg>
  </div>
</template>

<style scoped>
.line-node {
  position: relative;
  cursor: default;
  user-select: none;
}
.line-svg {
  display: block;
  overflow: visible;
}
</style>
