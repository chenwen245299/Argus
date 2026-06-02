<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { BaseEdge, getSmoothStepPath, useVueFlow, type EdgeProps } from '@vue-flow/core'

interface EdgeControlPoint {
  x: number
  y: number
}

interface AdjustableEdgeData {
  edgeColor?: string
  edgeStrokeWidth?: number
  controlX?: number
  controlY?: number
  controlPoints?: EdgeControlPoint[]
}

const props = defineProps<EdgeProps<AdjustableEdgeData>>()

const { screenToFlowCoordinate, updateEdgeData } = useVueFlow()

const draggingIndex = ref<number | null>(null)

function isValidPoint(point: unknown): point is EdgeControlPoint {
  if (!point || typeof point !== 'object') return false
  const maybe = point as Partial<EdgeControlPoint>
  return typeof maybe.x === 'number' && Number.isFinite(maybe.x) &&
    typeof maybe.y === 'number' && Number.isFinite(maybe.y)
}

const savedControlPoints = computed<EdgeControlPoint[]>(() => {
  if (Array.isArray(props.data?.controlPoints)) {
    return props.data.controlPoints.filter(isValidPoint)
  }

  const x = props.data?.controlX
  const y = props.data?.controlY
  if (typeof x === 'number' && Number.isFinite(x) && typeof y === 'number' && Number.isFinite(y)) {
    return [{ x, y }]
  }

  return []
})

const displayedControlPoints = computed<EdgeControlPoint[]>(() => {
  if (savedControlPoints.value.length > 0) return savedControlPoints.value
  const [, x, y] = defaultSmoothStepPath.value
  return [{
    x,
    y,
  }]
})

const pathPoints = computed<EdgeControlPoint[]>(() => [
  { x: props.sourceX, y: props.sourceY },
  ...displayedControlPoints.value,
  { x: props.targetX, y: props.targetY },
])

function catmullRomPath(points: EdgeControlPoint[]) {
  if (points.length === 0) return ''
  if (points.length === 1) return `M ${points[0].x},${points[0].y}`

  let path = `M ${points[0].x},${points[0].y}`
  for (let i = 0; i < points.length - 1; i += 1) {
    const p0 = points[i - 1] ?? points[i]
    const p1 = points[i]
    const p2 = points[i + 1]
    const p3 = points[i + 2] ?? p2
    const cp1 = {
      x: p1.x + (p2.x - p0.x) / 6,
      y: p1.y + (p2.y - p0.y) / 6,
    }
    const cp2 = {
      x: p2.x - (p3.x - p1.x) / 6,
      y: p2.y - (p3.y - p1.y) / 6,
    }
    path += ` C ${cp1.x},${cp1.y} ${cp2.x},${cp2.y} ${p2.x},${p2.y}`
  }
  return path
}

function distanceToSegment(point: EdgeControlPoint, start: EdgeControlPoint, end: EdgeControlPoint) {
  const dx = end.x - start.x
  const dy = end.y - start.y
  const lengthSq = dx * dx + dy * dy
  if (lengthSq === 0) return Math.hypot(point.x - start.x, point.y - start.y)

  const t = Math.max(0, Math.min(1, ((point.x - start.x) * dx + (point.y - start.y) * dy) / lengthSq))
  const projection = { x: start.x + t * dx, y: start.y + t * dy }
  return Math.hypot(point.x - projection.x, point.y - projection.y)
}

function nearestInsertIndex(point: EdgeControlPoint) {
  const points = [
    { x: props.sourceX, y: props.sourceY },
    ...savedControlPoints.value,
    { x: props.targetX, y: props.targetY },
  ]
  let bestSegment = 0
  let bestDistance = Number.POSITIVE_INFINITY

  for (let i = 0; i < points.length - 1; i += 1) {
    const distance = distanceToSegment(point, points[i], points[i + 1])
    if (distance < bestDistance) {
      bestDistance = distance
      bestSegment = i
    }
  }

  return bestSegment
}

function saveControlPoints(points: EdgeControlPoint[]) {
  updateEdgeData<AdjustableEdgeData>(props.id, {
    controlPoints: points,
    controlX: undefined,
    controlY: undefined,
  })
}

const defaultSmoothStepPath = computed(() => getSmoothStepPath({
  sourceX: props.sourceX,
  sourceY: props.sourceY,
  sourcePosition: props.sourcePosition,
  targetX: props.targetX,
  targetY: props.targetY,
  targetPosition: props.targetPosition,
}))

const edgePath = computed(() => {
  if (savedControlPoints.value.length === 0) return defaultSmoothStepPath.value[0]
  return catmullRomPath(pathPoints.value)
})

const labelX = computed(() => {
  if (savedControlPoints.value.length === 0) return defaultSmoothStepPath.value[1]
  const points = pathPoints.value
  return points.reduce((sum, point) => sum + point.x, 0) / points.length
})

const labelY = computed(() => {
  if (savedControlPoints.value.length === 0) return defaultSmoothStepPath.value[2]
  const points = pathPoints.value
  return points.reduce((sum, point) => sum + point.y, 0) / points.length
})

function pointFromEvent(event: MouseEvent | PointerEvent) {
  return screenToFlowCoordinate({ x: event.clientX, y: event.clientY })
}

function setControlFromEvent(index: number, event: PointerEvent) {
  const points = [...savedControlPoints.value]
  points[index] = pointFromEvent(event)
  saveControlPoints(points)
}

function cleanupDragListeners() {
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', onPointerUp)
}

function onPointerMove(event: PointerEvent) {
  if (draggingIndex.value === null) return
  event.preventDefault()
  setControlFromEvent(draggingIndex.value, event)
}

function onPointerUp(event: PointerEvent) {
  if (draggingIndex.value === null) return
  event.preventDefault()
  setControlFromEvent(draggingIndex.value, event)
  draggingIndex.value = null
  cleanupDragListeners()
  window.dispatchEvent(new CustomEvent('argus-canvas-edge-control-changed', {
    detail: { edgeId: props.id },
  }))
}

function onControlPointerDown(index: number, event: PointerEvent) {
  if (event.button !== 0) return
  event.preventDefault()
  event.stopPropagation()
  if (savedControlPoints.value.length === 0) {
    saveControlPoints([...displayedControlPoints.value])
  }
  draggingIndex.value = index
  setControlFromEvent(index, event)
  window.addEventListener('pointermove', onPointerMove)
  window.addEventListener('pointerup', onPointerUp)
}

function onEdgeDblClick(event: MouseEvent) {
  event.preventDefault()
  event.stopPropagation()
  const point = pointFromEvent(event)
  const points = [...savedControlPoints.value]
  points.splice(nearestInsertIndex(point), 0, point)
  saveControlPoints(points)
  window.dispatchEvent(new CustomEvent('argus-canvas-edge-control-changed', {
    detail: { edgeId: props.id },
  }))
}

onUnmounted(cleanupDragListeners)
</script>

<template>
  <g
    class="adjustable-edge"
    :class="{ 'adjustable-edge--selected': props.selected, 'adjustable-edge--dragging': draggingIndex !== null }"
    @dblclick="onEdgeDblClick"
  >
    <BaseEdge
      :id="props.id"
      :path="edgePath"
      :label="props.label"
      :label-x="labelX"
      :label-y="labelY"
      :label-style="props.labelStyle"
      :label-show-bg="props.labelShowBg"
      :label-bg-style="props.labelBgStyle"
      :label-bg-padding="props.labelBgPadding"
      :label-bg-border-radius="props.labelBgBorderRadius"
      :marker-start="props.markerStart"
      :marker-end="props.markerEnd"
      :interaction-width="props.interactionWidth ?? 24"
      :style="props.style"
    />
    <g
      v-for="(point, index) in displayedControlPoints"
      :key="`${index}-${point.x}-${point.y}`"
      class="adjustable-edge-control nodrag nopan"
      :class="{ 'adjustable-edge-control--implicit': savedControlPoints.length === 0 }"
      :transform="`translate(${point.x} ${point.y})`"
      @pointerdown="onControlPointerDown(index, $event)"
    >
      <circle class="adjustable-edge-control-hit" r="12" />
      <circle class="adjustable-edge-control-dot" r="5" />
    </g>
  </g>
</template>

<style scoped>
.adjustable-edge-control {
  opacity: 0;
  pointer-events: none;
  cursor: grab;
  transition: opacity 0.12s ease;
}

.adjustable-edge:hover .adjustable-edge-control,
.adjustable-edge--selected .adjustable-edge-control,
.adjustable-edge--dragging .adjustable-edge-control {
  opacity: 1;
  pointer-events: all;
}

.adjustable-edge--dragging .adjustable-edge-control {
  cursor: grabbing;
}

.adjustable-edge-control-hit {
  fill: transparent;
}

.adjustable-edge-control-dot {
  fill: var(--bg-primary, #fff);
  stroke: var(--accent, #1677ff);
  stroke-width: 2;
  filter: drop-shadow(0 1px 3px rgba(0, 0, 0, 0.2));
}

.adjustable-edge-control--implicit .adjustable-edge-control-dot {
  stroke-dasharray: 2 2;
}
</style>
