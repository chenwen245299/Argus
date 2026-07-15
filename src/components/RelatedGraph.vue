<script setup lang="ts">
// Compact radial graph of a paper's manual "similar papers" network. The root
// paper sits at the center; the full multi-hop network (similar-of-similar) is
// shown by default in a calm style. Single-click a node to focus it — it and
// its direct neighbours highlight while the rest dim. Double-click opens.
// Edges are SVG; nodes are HTML cards sharing the same pan/zoom transform so a
// card can show the full, wrapped title with real CSS.
import { ref, computed, reactive, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLibraryStore } from '../stores/library'
import type { PaperIndexEntry } from '../types'

const props = defineProps<{ rootSlug: string }>()
const emit = defineEmits<{ (e: 'open', entry: PaperIndexEntry): void }>()

const { t } = useI18n()
const library = useLibraryStore()

// The drawing surface tracks the actual viewport size so the graph fills
// whatever space the popover gives it (normal or enlarged).
const vpEl = ref<HTMLElement | null>(null)
const W = ref(340)
const H = ref(300)
const CX = computed(() => W.value / 2)
const CY = computed(() => H.value / 2)
const RING = 190          // radius added per hop from the center
const CARD_W = 172        // used only for the auto-fit bounding box
const CARD_H = 62

let resizeObs: ResizeObserver | null = null
function measureViewport() {
  const el = vpEl.value
  if (!el) return
  W.value = el.clientWidth || 340
  H.value = el.clientHeight || 300
}
onMounted(() => {
  measureViewport()
  if (vpEl.value && typeof ResizeObserver !== 'undefined') {
    resizeObs = new ResizeObserver(measureViewport)
    resizeObs.observe(vpEl.value)
  }
})
onBeforeUnmount(() => { resizeObs?.disconnect() })
watch([W, H], () => fitView())
const MAX_NODES = 60      // safety caps so a dense network can't explode
const MAX_DEPTH = 4

const rootId = computed(() => library.papers.find(p => p.slug === props.rootSlug)?.id ?? '')

function entryById(id: string): PaperIndexEntry | undefined {
  return library.papers.find(p => p.id === id)
}
function relatedIdsOf(id: string): string[] {
  return (entryById(id)?.related_ids ?? []).filter(rid => !!entryById(rid))
}

// Full reachable multi-hop network from the root (capped by depth + node count).
const visibleIds = computed(() => {
  const rid = rootId.value
  const vis = new Set<string>()
  if (!rid) return vis
  vis.add(rid)
  let frontier = [rid]
  let depth = 0
  while (frontier.length && depth < MAX_DEPTH && vis.size < MAX_NODES) {
    const next: string[] = []
    for (const id of frontier) {
      for (const c of relatedIdsOf(id)) {
        if (vis.has(c)) continue
        vis.add(c)
        next.push(c)
        if (vis.size >= MAX_NODES) break
      }
      if (vis.size >= MAX_NODES) break
    }
    frontier = next
    depth++
  }
  return vis
})

// ── Radial tree layout ──────────────────────────────────────────────────────
const layout = computed(() => {
  const posMap: Record<string, { x: number; y: number }> = {}
  const rid = rootId.value
  const vis = visibleIds.value
  if (!rid || !vis.has(rid)) return posMap

  const children: Record<string, string[]> = {}
  const order: string[] = [rid]
  const seen = new Set([rid])
  const queue = [rid]
  while (queue.length) {
    const cur = queue.shift()!
    children[cur] = children[cur] || []
    for (const c of relatedIdsOf(cur)) {
      if (!vis.has(c) || seen.has(c)) continue
      seen.add(c)
      children[cur].push(c)
      queue.push(c)
      order.push(c)
    }
  }

  const leaves: Record<string, number> = {}
  for (let i = order.length - 1; i >= 0; i--) {
    const id = order[i]
    const ch = children[id] || []
    leaves[id] = ch.length ? ch.reduce((s, c) => s + leaves[c], 0) : 1
  }

  function place(id: string, a0: number, a1: number, depth: number) {
    const ang = (a0 + a1) / 2
    posMap[id] = depth === 0
      ? { x: 0, y: 0 }
      : { x: Math.cos(ang) * depth * RING, y: Math.sin(ang) * depth * RING }
    const ch = children[id] || []
    if (!ch.length) return
    let a = a0
    for (const c of ch) {
      const span = (a1 - a0) * (leaves[c] / leaves[id])
      place(c, a, a + span, depth + 1)
      a += span
    }
  }
  place(rid, -Math.PI / 2, Math.PI * 1.5, 0)
  return posMap
})

const nodeList = computed(() =>
  [...visibleIds.value]
    .filter(id => layout.value[id])
    .map(id => ({ id, x: layout.value[id].x, y: layout.value[id].y })))

const links = computed(() => {
  const out: { s: string; t: string }[] = []
  const seen = new Set<string>()
  const vis = visibleIds.value
  for (const id of vis) {
    for (const rid of relatedIdsOf(id)) {
      if (!vis.has(rid)) continue
      const key = id < rid ? `${id}|${rid}` : `${rid}|${id}`
      if (seen.has(key)) continue
      seen.add(key)
      out.push({ s: id, t: rid })
    }
  }
  return out
})
function pos(id: string) { return layout.value[id] ?? { x: 0, y: 0 } }

// ── Focus (single-click to highlight a node + its neighbours) ────────────────
// Exactly one node is highlighted at a time; default to the anchor (root).
const selectedId = ref<string | null>(null)
watch(rootId, (v) => { selectedId.value = v || null }, { immediate: true })

const neighborIds = computed(() => {
  const id = selectedId.value
  const s = new Set<string>()
  if (!id) return s
  s.add(id)
  for (const l of links.value) {
    if (l.s === id) s.add(l.t)
    if (l.t === id) s.add(l.s)
  }
  return s
})

function cardClass(id: string) {
  const sel = selectedId.value
  return {
    root: id === rootId.value,
    selected: sel === id,
    neighbor: !!sel && sel !== id && neighborIds.value.has(id),
    dim: !!sel && !neighborIds.value.has(id),
  }
}
function linkClass(l: { s: string; t: string }) {
  if (!selectedId.value) return ''
  return (l.s === selectedId.value || l.t === selectedId.value) ? 'hi' : 'dim'
}

function fullTitle(id: string) { return entryById(id)?.title ?? '' }
function subLine(id: string) {
  const e = entryById(id)
  if (!e) return ''
  const first = e.authors?.[0] ?? ''
  const etal = (e.authors?.length ?? 0) > 1 ? ' et al.' : ''
  const yr = e.year ? ` · ${e.year}` : ''
  return `${first}${etal}${yr}`
}

// ── Pan / zoom, auto-fit on layout change ────────────────────────────────────
const view = reactive({ x: 0, y: 0, k: 1 })

function fitView() {
  const ids = Object.keys(layout.value)
  if (!ids.length) { view.x = 0; view.y = 0; view.k = 1; return }
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity
  for (const id of ids) {
    const p = layout.value[id]
    minX = Math.min(minX, p.x - CARD_W / 2); maxX = Math.max(maxX, p.x + CARD_W / 2)
    minY = Math.min(minY, p.y - CARD_H / 2); maxY = Math.max(maxY, p.y + CARD_H / 2)
  }
  const pad = 14
  const k = Math.min(W.value / (maxX - minX + pad * 2), H.value / (maxY - minY + pad * 2), 1)
  view.k = k
  view.x = -((minX + maxX) / 2) * k
  view.y = -((minY + maxY) / 2) * k
}
watch(layout, fitView, { immediate: true })

let panning = false
let panStart = { x: 0, y: 0 }
let viewStart = { x: 0, y: 0 }
function onBgDown(e: MouseEvent) {
  selectedId.value = rootId.value || null   // clicking empty space refocuses the anchor
  panning = true
  panStart = { x: e.clientX, y: e.clientY }
  viewStart = { x: view.x, y: view.y }
  window.addEventListener('mousemove', onBgMove)
  window.addEventListener('mouseup', onBgUp)
}
function onBgMove(e: MouseEvent) {
  if (!panning) return
  view.x = viewStart.x + (e.clientX - panStart.x)
  view.y = viewStart.y + (e.clientY - panStart.y)
}
function onBgUp() {
  panning = false
  window.removeEventListener('mousemove', onBgMove)
  window.removeEventListener('mouseup', onBgUp)
}
function onWheel(e: WheelEvent) {
  e.preventDefault()
  view.k = Math.min(2.4, Math.max(0.35, view.k * (e.deltaY < 0 ? 1.12 : 0.89)))
}
onBeforeUnmount(onBgUp)

const edgeTransform = computed(() =>
  `translate(${CX.value + view.x}, ${CY.value + view.y}) scale(${view.k})`)
const nodesTransform = computed(() =>
  `translate(${CX.value + view.x}px, ${CY.value + view.y}px) scale(${view.k})`)

function onNodeClick(id: string) {
  selectedId.value = id
}
function onNodeDblClick(id: string) {
  const e = entryById(id)
  if (e) emit('open', e)
}

const isEmpty = computed(() => visibleIds.value.size <= 1)
</script>

<template>
  <div class="related-graph">
    <div
      ref="vpEl"
      class="rg-viewport"
      :style="{ backgroundPosition: `${view.x}px ${view.y}px` }"
      @mousedown="onBgDown"
      @wheel="onWheel"
    >
      <svg class="rg-edges" :width="W" :height="H" :viewBox="`0 0 ${W} ${H}`">
        <g :transform="edgeTransform">
          <line
            v-for="(l, i) in links"
            :key="`l${i}`"
            class="rg-link"
            :class="linkClass(l)"
            :x1="pos(l.s).x"
            :y1="pos(l.s).y"
            :x2="pos(l.t).x"
            :y2="pos(l.t).y"
          />
        </g>
      </svg>

      <div class="rg-nodes" :style="{ transform: nodesTransform }">
        <div
          v-for="n in nodeList"
          :key="n.id"
          class="rg-card"
          :class="cardClass(n.id)"
          :style="{ transform: `translate(${n.x}px, ${n.y}px) translate(-50%, -50%)` }"
          :title="fullTitle(n.id)"
          @mousedown.stop
          @click.stop="onNodeClick(n.id)"
          @dblclick.stop="onNodeDblClick(n.id)"
        >
          <span class="rg-card-title">{{ fullTitle(n.id) }}</span>
          <span v-if="subLine(n.id)" class="rg-card-sub">{{ subLine(n.id) }}</span>
        </div>
      </div>

      <div v-if="isEmpty" class="rg-empty">{{ t('related.empty') }}</div>
    </div>

    <div class="rg-footer">
      <span class="rg-hint">{{ t('related.graphHint') }}</span>
      <button class="rg-reset" @click="fitView">{{ t('related.resetView') }}</button>
    </div>
  </div>
</template>

<style scoped>
.related-graph {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.rg-viewport {
  position: relative;
  width: 100%;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
  background-color: var(--bg-secondary);
  background-image: radial-gradient(
    color-mix(in srgb, var(--text-tertiary) 22%, transparent) 1px,
    transparent 1px
  );
  background-size: 18px 18px;
  cursor: grab;
  user-select: none;
}
.rg-viewport:active { cursor: grabbing; }

.rg-edges {
  position: absolute;
  inset: 0;
  pointer-events: none;
}
.rg-link {
  stroke: color-mix(in srgb, var(--accent) 26%, var(--border-default));
  stroke-width: 1.5;
  stroke-linecap: round;
  transition: stroke 0.15s, opacity 0.15s;
}
.rg-link.hi {
  stroke: var(--accent);
  stroke-width: 2;
}
.rg-link.dim { opacity: 0.18; }

.rg-nodes {
  position: absolute;
  inset: 0;
  transform-origin: 0 0;
}

.rg-card {
  position: absolute;
  left: 0;
  top: 0;
  width: 168px;
  box-sizing: border-box;
  padding: 8px 11px;
  border-radius: 12px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.06), 0 3px 10px rgba(0, 0, 0, 0.05);
  cursor: pointer;
  transition: box-shadow 0.15s, border-color 0.15s, opacity 0.18s, transform 0.05s;
  animation: rg-pop 0.22s ease both;
}
.rg-card:hover {
  border-color: var(--accent);
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.08), 0 6px 18px color-mix(in srgb, var(--accent) 18%, transparent);
  z-index: 5;
}
@keyframes rg-pop {
  from { opacity: 0; }
  to   { opacity: 1; }
}

.rg-card-title {
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  font-size: 11.5px;
  line-height: 1.35;
  font-weight: 500;
  color: var(--text-primary);
}
.rg-card-sub {
  display: block;
  margin-top: 3px;
  font-size: 10px;
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* Root (anchor): slightly larger so the centre is easy to find. Its colour
   comes only from the single-highlight (selected) state below. */
.rg-card.root { width: 176px; }

/* Exactly one node is highlighted at a time: the selected node turns solid
   accent (the "core"), its neighbours get an accent border to stand out, and
   everything else dims. */
.rg-card.neighbor {
  border-color: color-mix(in srgb, var(--accent) 45%, var(--border-default));
}
.rg-card.selected {
  background: var(--accent);
  border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 28%, transparent),
    0 6px 18px color-mix(in srgb, var(--accent) 30%, transparent);
  z-index: 6;
}
.rg-card.selected .rg-card-title { color: #fff; font-weight: 600; }
.rg-card.selected .rg-card-sub { color: rgba(255, 255, 255, 0.82); }
.rg-card.dim { opacity: 0.3; }

.rg-empty {
  position: absolute;
  top: 50%;
  left: 0;
  right: 0;
  transform: translateY(-50%);
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  pointer-events: none;
}

.rg-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 8px;
}
.rg-hint {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.rg-reset {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
}
.rg-reset:hover { background: var(--bg-hover); color: var(--text-primary); }
</style>
