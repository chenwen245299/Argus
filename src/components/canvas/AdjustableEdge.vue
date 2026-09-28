<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { BaseEdge, getSmoothStepPath, useVueFlow, type EdgeProps } from '@vue-flow/core'
import {
  EDGE_CORNER_RADIUS,
  EDGE_HANDLE_OFFSET,
  JOG,
  closestPointOnLine,
  insertControlPoint,
  placeControlPoint,
  planImplicitDrag,
  polylineMidpoint,
  polylinePath,
  readControlPoints,
  routeOrthogonal,
  smoothStepVertices,
  type DragPlan,
  type EdgeEnds,
  type RoutePoint,
} from '../../utils/orthogonalRoute'

// Every edge is a polyline, never a curve: the smooth-step default until the user
// places a control point, then an orthogonal route through the points they placed
// (see utils/orthogonalRoute). Control points saved while edges were still drawn
// as curves are kept and simply routed this way.

interface AdjustableEdgeData {
  edgeColor?: string
  edgeStrokeWidth?: number
  controlX?: number
  controlY?: number
  controlPoints?: RoutePoint[]
}

const props = defineProps<EdgeProps<AdjustableEdgeData>>()

const { screenToFlowCoordinate, updateEdgeData, viewport } = useVueFlow()

/** Screen pixels a press must travel before it drags, so a click or double-click doesn't nudge the point. */
const DRAG_THRESHOLD_PX = 3
/** Screen pixels within which a dragged point snaps onto a neighbour's row or column. */
const SNAP_PX = 6
/** Two presses on the same point within this many ms, without a drag, remove it. */
const DOUBLE_PRESS_MS = 350

interface ActiveDrag extends DragPlan {
  /** Where the grabbed point started; the points in `follow` move by its offset from here. */
  start: RoutePoint
  /** The settled points either side of it (null for a handle), which it snaps to. */
  prev: RoutePoint | null
  next: RoutePoint | null
}

const draggingIndex = ref<number | null>(null)
let drag: ActiveDrag | null = null
let grabOffset: RoutePoint = { x: 0, y: 0 }
let pressStart = { x: 0, y: 0 }
let dragStarted = false
let lastPress: { index: number; at: number } | null = null

const savedControlPoints = computed<RoutePoint[]>(() => readControlPoints(props.data))

const ends = computed<EdgeEnds>(() => ({
  source: { x: props.sourceX, y: props.sourceY },
  sourceSide: props.sourcePosition,
  target: { x: props.targetX, y: props.targetY },
  targetSide: props.targetPosition,
}))

const defaultSmoothStepPath = computed(() => getSmoothStepPath({
  sourceX: props.sourceX,
  sourceY: props.sourceY,
  sourcePosition: props.sourcePosition,
  targetX: props.targetX,
  targetY: props.targetY,
  targetPosition: props.targetPosition,
  borderRadius: EDGE_CORNER_RADIUS,
  offset: EDGE_HANDLE_OFFSET,
}))

/** The vertices of the default path, which editing an edge without control points starts from. */
const defaultLine = computed(() => smoothStepVertices(defaultSmoothStepPath.value[0]))

/** The routed line once the user has placed control points; null while the default path is drawn. */
const route = computed(() => (
  savedControlPoints.value.length > 0
    ? routeOrthogonal({ ...ends.value, waypoints: savedControlPoints.value })
    : null
))

/**
 * Handles sit on the line: at each control point as routed, or — with none
 * saved — one in the middle of the default path, ready to drag.
 */
const displayedControlPoints = computed<RoutePoint[]>(() => {
  const current = route.value
  if (current) return current.waypoints.map(point => closestPointOnLine(current.points, point))
  const [, x, y] = defaultSmoothStepPath.value
  return [{ x, y }]
})

const edgePath = computed(() => (
  route.value ? polylinePath(route.value.points) : defaultSmoothStepPath.value[0]
))

const labelPoint = computed<RoutePoint>(() => {
  if (route.value) return polylineMidpoint(route.value.points)
  const [, x, y] = defaultSmoothStepPath.value
  return { x, y }
})

function saveControlPoints(points: RoutePoint[]) {
  updateEdgeData<AdjustableEdgeData>(props.id, {
    controlPoints: points,
    controlX: undefined,
    controlY: undefined,
  })
}

function announceChange() {
  window.dispatchEvent(new CustomEvent('argus-canvas-edge-control-changed', {
    detail: { edgeId: props.id },
  }))
}

function pointFromEvent(event: MouseEvent | PointerEvent) {
  return screenToFlowCoordinate({ x: event.clientX, y: event.clientY })
}

function planDrag(index: number): ActiveDrag {
  const current = route.value
  if (current) {
    // The points as routed, so what's saved is what's drawn.
    return {
      points: [...current.waypoints],
      index,
      follow: [],
      across: null,
      slide: null,
      start: displayedControlPoints.value[index],
      prev: current.waypoints[index - 1] ?? null,
      next: current.waypoints[index + 1] ?? null,
    }
  }
  // Grabbing the default path's handle: turn it into control points that draw
  // the same line, and snap it against the nearest ones that don't move with it.
  const plan = planImplicitDrag(ends.value, defaultLine.value, displayedControlPoints.value[0])
  const still = (i: number) => !plan.follow.includes(i)
  const before = plan.points.slice(0, plan.index).reverse().find((_, k) => still(plan.index - 1 - k))
  const after = plan.points.slice(plan.index + 1).find((_, k) => still(plan.index + 1 + k))
  return { ...plan, start: plan.points[plan.index], prev: before ?? null, next: after ?? null }
}

function moveControlTo(event: PointerEvent) {
  if (!drag) return
  const { index, start, across, follow, slide } = drag
  const pointer = pointFromEvent(event)
  const raw = { x: pointer.x + grabOffset.x, y: pointer.y + grabOffset.y }
  // At least JOG, so a dragged point always lands exactly where routing would settle it.
  const tolerance = Math.max(SNAP_PX / (viewport.value.zoom || 1), JOG)
  const placed = placeControlPoint(raw, drag.prev, drag.next, ends.value, tolerance)
  // A point whose run ends are pinned stays on its run, or the line would double back.
  if (slide) placed[slide.axis] = Math.min(Math.max(placed[slide.axis], slide.min), slide.max)
  saveControlPoints(drag.points.map((point, i) => {
    if (i === index) return placed
    if (across && follow.includes(i)) {
      return across === 'x'
        ? { x: point.x + placed.x - start.x, y: point.y }
        : { x: point.x, y: point.y + placed.y - start.y }
    }
    return point
  }))
}

function cleanupDragListeners() {
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', onPointerUp)
}

function onPointerMove(event: PointerEvent) {
  if (draggingIndex.value === null) return
  event.preventDefault()
  if (!dragStarted) {
    const travelled = Math.hypot(event.clientX - pressStart.x, event.clientY - pressStart.y)
    if (travelled < DRAG_THRESHOLD_PX) return
    dragStarted = true
    drag = planDrag(draggingIndex.value)
  }
  moveControlTo(event)
}

function onPointerUp(event: PointerEvent) {
  const index = draggingIndex.value
  if (index === null) return
  event.preventDefault()
  draggingIndex.value = null
  cleanupDragListeners()
  if (dragStarted) {
    moveControlTo(event)
    drag = null
    lastPress = null
    announceChange()
    return
  }
  // A press that never became a drag: the second of two in quick succession on
  // a point the user placed removes it (the default path's handle has nothing to remove).
  const now = event.timeStamp
  if (lastPress && lastPress.index === index && now - lastPress.at < DOUBLE_PRESS_MS) {
    lastPress = null
    if (savedControlPoints.value.length > 0) {
      saveControlPoints(savedControlPoints.value.filter((_, i) => i !== index))
      announceChange()
    }
    return
  }
  lastPress = { index, at: now }
}

function onControlPointerDown(index: number, event: PointerEvent) {
  if (event.button !== 0) return
  event.preventDefault()
  event.stopPropagation()
  const start = displayedControlPoints.value[index]
  const pointer = pointFromEvent(event)
  // Drag by the grab offset, so pressing near the edge of the hit circle doesn't jump the point.
  grabOffset = { x: start.x - pointer.x, y: start.y - pointer.y }
  pressStart = { x: event.clientX, y: event.clientY }
  dragStarted = false
  drag = null
  draggingIndex.value = index
  window.addEventListener('pointermove', onPointerMove)
  window.addEventListener('pointerup', onPointerUp)
}

function onEdgeDblClick(event: MouseEvent) {
  event.preventDefault()
  event.stopPropagation()
  saveControlPoints(insertControlPoint(
    ends.value,
    savedControlPoints.value,
    defaultLine.value,
    pointFromEvent(event),
  ))
  announceChange()
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
      :label-x="labelPoint.x"
      :label-y="labelPoint.y"
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
      @dblclick.stop.prevent
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
