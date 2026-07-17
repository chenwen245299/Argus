<script setup lang="ts">
// Force-directed citation graph. The current paper is a fixed centre; the papers
// it cites are physics bodies linked to it by springs, repelling each other and
// resolving collisions so nodes never overlap. Clicking an in-library node
// fetches ITS references and springs them out FROM that node (the view never
// jumps). Nodes are draggable. A reference cited by several sources is drawn as
// an equal pie of those sources' colours. Node size scales with citation count.
import { ref, computed, reactive, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import type { CitationRef } from '../types'

const props = defineProps<{
  centerTitle: string
  centerCiteCount?: number | null
  refs: CitationRef[]
  maxNodes?: number
}>()
const emit = defineEmits<{ (e: 'open', slug: string): void }>()

const { t } = useI18n()

const rootLimit = computed(() => Math.max(1, Math.round(props.maxNodes ?? 36)))
const expandLimit = computed(() => Math.min(rootLimit.value, 30))
const BASE_D = 34
const MAX_D = 96
const CENTER_D = 80
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5))
const MIN_FIT_K = 0.5

// Physics tuning — spread out generously so nodes/labels don't crowd.
const LINK_DIST = 230
const LINK_STRENGTH = 0.05
const CHARGE = -2800
const CENTER_GRAVITY = 0.012
const COLLIDE_PAD = 24
const FRICTION = 0.72
const ALPHA_DECAY = 0.024
const ALPHA_MIN = 0.02

const CENTER_KEY = '__center__'
const ROOT_ID = '__root__'
const ROOT_COLOR = '#2563eb'
const SOURCE_COLORS = ['#ea580c', '#16a34a', '#7c3aed', '#db2777', '#0891b2', '#ca8a04', '#0d9488', '#9333ea']

interface Source { id: string; title: string; color: string; hubKey: string }
interface GNode {
  key: string; ref: CitationRef | null; isCenter?: boolean
  x: number; y: number; vx: number; vy: number
  fx: number | null; fy: number | null   // pinned position (drag / centre)
  sourceIds: string[]
}

const sources = ref<Source[]>([])
const simNodes: GNode[] = []                 // plain physics bodies
const nodeIndex = new Map<string, GNode>()
const version = ref(0)                        // bump each tick to re-render
const expandingId = ref<string | null>(null)
const selectedKey = ref<string | null>(null)
const copied = ref(false)
let maxCiteVal = 1

// Simulation state — declared before rebuildBase so its immediate watch (which
// calls reheat()) doesn't hit a temporal-dead-zone on these.
let alpha = 0
let raf = 0
let running = false
let dragging = false
let pendingFit = false
let buildGen = 0        // bumped on rebuild/reset to abort a running staggered add
let alive = true        // false after unmount
const STAGGER_MS = 50   // delay between each node appearing on expand

function refKey(r: CitationRef): string {
  if (r.paper_id) return 'id:' + r.paper_id
  if (r.doi) return 'doi:' + r.doi.trim().toLowerCase()
  return 'ti:' + r.title.toLowerCase().replace(/[^a-z0-9]+/g, '')
}
const byCiteDesc = (a: CitationRef, b: CitationRef) => (b.cite_count ?? -1) - (a.cite_count ?? -1)
const sourceColor = (id: string) => sources.value.find(s => s.id === id)?.color ?? '#94a3b8'

function diamOf(c?: number | null) {
  if (c == null) return BASE_D
  return BASE_D + (Math.log(c + 1) / Math.log(maxCiteVal + 1)) * (MAX_D - BASE_D)
}
function radiusOf(n: GNode) {
  return (n.isCenter ? CENTER_D : diamOf(n.ref?.cite_count)) / 2 + COLLIDE_PAD
}
function recomputeMaxCite() {
  maxCiteVal = Math.max(1, ...simNodes.filter(n => !n.isCenter).map(n => n.ref?.cite_count ?? 0))
}

// ── Build / expand ───────────────────────────────────────────────────────────
function resetSim() {
  simNodes.length = 0
  nodeIndex.clear()
  const center: GNode = { key: CENTER_KEY, ref: null, isCenter: true, x: 0, y: 0, vx: 0, vy: 0, fx: 0, fy: 0, sourceIds: [] }
  simNodes.push(center)
  nodeIndex.set(CENTER_KEY, center)
}

function rebuildBase() {
  buildGen++     // abort any in-flight staggered expansion from a previous state
  resetSim()
  sources.value = [{ id: ROOT_ID, title: props.centerTitle, color: ROOT_COLOR, hubKey: CENTER_KEY }]
  const sorted = [...props.refs].sort(byCiteDesc).slice(0, rootLimit.value)
  sorted.forEach((r, i) => {
    const ang = i * GOLDEN_ANGLE
    const rad = 150 + 16 * Math.sqrt(i)
    const k = refKey(r)
    const node: GNode = { key: k, ref: r, x: Math.cos(ang) * rad, y: Math.sin(ang) * rad, vx: 0, vy: 0, fx: null, fy: null, sourceIds: [ROOT_ID] }
    simNodes.push(node)
    nodeIndex.set(k, node)
  })
  recomputeMaxCite()
  selectedKey.value = null
  pendingFit = true
  reheat(1)
}
watch(() => [props.refs, props.maxNodes], rebuildBase, { immediate: true })

async function expand(node: GNode) {
  const slug = node.ref?.library_slug
  if (!slug || sources.value.some(s => s.id === slug) || expandingId.value) return
  expandingId.value = slug
  try {
    let refs = await invoke<CitationRef[]>('get_cached_references', { slug })
    if (!refs.length) refs = await invoke<CitationRef[]>('fetch_references', { slug })
    addSource(slug, node.ref!.title, node, refs)
  } catch (e) {
    console.error('Failed to expand citations:', e)
  } finally {
    expandingId.value = null
  }
}

function addSource(id: string, title: string, hub: GNode, refs: CitationRef[]) {
  const color = SOURCE_COLORS[(sources.value.length - 1) % SOURCE_COLORS.length]
  sources.value = [...sources.value, { id, title, color, hubKey: hub.key }]
  const sorted = [...refs].sort(byCiteDesc).slice(0, expandLimit.value)
  const gen = buildGen
  let i = 0
  // Add nodes one at a time so the user watches them cascade out of the clicked
  // node, rather than all appearing at once.
  const step = () => {
    if (!alive || gen !== buildGen) return
    if (i >= sorted.length) { recomputeMaxCite(); reheat(0.4); return }
    const r = sorted[i]
    i++
    const k = refKey(r)
    const existing = nodeIndex.get(k)
    if (existing) {
      if (!existing.sourceIds.includes(id)) existing.sourceIds.push(id)
    } else {
      // Born AT the hub (tiny scatter) then springs outward via the physics.
      const ang = i * GOLDEN_ANGLE
      const node: GNode = {
        key: k, ref: r,
        x: hub.x + Math.cos(ang) * (14 + i), y: hub.y + Math.sin(ang) * (14 + i),
        vx: 0, vy: 0, fx: null, fy: null, sourceIds: [id],
      }
      simNodes.push(node)
      nodeIndex.set(k, node)
    }
    recomputeMaxCite()
    reheat(0.6)          // keep physics warm so each new node springs out
    version.value++
    setTimeout(step, STAGGER_MS)
  }
  step()
}

const hiddenCount = computed(() => Math.max(0, props.refs.length - rootLimit.value))

// ── Physics simulation ───────────────────────────────────────────────────────
function reheat(target = 0.6) {
  alpha = Math.max(alpha, target)
  if (!running) { running = true; raf = requestAnimationFrame(tick) }
}

function tick() {
  const nodes = simNodes
  const n = nodes.length
  const srcById = new Map(sources.value.map(s => [s.id, s]))

  // Link springs: pull each node toward its source hub(s).
  for (const nd of nodes) {
    if (nd.isCenter) continue
    for (const sid of nd.sourceIds) {
      const s = srcById.get(sid); if (!s) continue
      const hub = nodeIndex.get(s.hubKey); if (!hub || hub === nd) continue
      const dx = nd.x - hub.x, dy = nd.y - hub.y
      const d = Math.hypot(dx, dy) || 0.01
      const k = ((d - LINK_DIST) / d) * LINK_STRENGTH * alpha
      nd.vx -= dx * k; nd.vy -= dy * k
      if (!hub.isCenter && hub.fx == null) { hub.vx += dx * k; hub.vy += dy * k }
    }
  }

  // Charge: pairwise repulsion.
  for (let i = 0; i < n; i++) {
    const a = nodes[i]
    for (let j = i + 1; j < n; j++) {
      const b = nodes[j]
      let dx = a.x - b.x, dy = a.y - b.y
      let d2 = dx * dx + dy * dy
      if (d2 < 1) { d2 = 1; dx = (i - j) || 1; dy = 1 }
      const d = Math.sqrt(d2)
      const f = (CHARGE * alpha) / d2
      const fx = (dx / d) * f, fy = (dy / d) * f
      a.vx -= fx; a.vy -= fy
      b.vx += fx; b.vy += fy
    }
  }

  // Gravity toward the centre.
  for (const nd of nodes) {
    if (nd.isCenter) continue
    nd.vx += -nd.x * CENTER_GRAVITY * alpha
    nd.vy += -nd.y * CENTER_GRAVITY * alpha
  }

  // Integrate (pinned nodes stay put).
  for (const nd of nodes) {
    if (nd.fx != null) { nd.x = nd.fx; nd.vx = 0 } else { nd.vx *= FRICTION; nd.x += nd.vx }
    if (nd.fy != null) { nd.y = nd.fy; nd.vy = 0 } else { nd.vy *= FRICTION; nd.y += nd.vy }
  }

  // Collision: hard-resolve overlaps so nodes never sit on top of each other.
  for (let iter = 0; iter < 2; iter++) {
    for (let i = 0; i < n; i++) {
      const a = nodes[i]
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j]
        let dx = b.x - a.x, dy = b.y - a.y
        let d = Math.hypot(dx, dy)
        if (d === 0) { dx = 0.5; dy = 0.5; d = 0.707 }
        const min = radiusOf(a) + radiusOf(b)
        if (d < min) {
          const push = (min - d) / d * 0.5
          const ox = dx * push, oy = dy * push
          const aFixed = a.fx != null || a.isCenter
          const bFixed = b.fx != null || b.isCenter
          if (aFixed && bFixed) continue
          if (!aFixed && !bFixed) { a.x -= ox; a.y -= oy; b.x += ox; b.y += oy }
          else if (aFixed) { b.x += ox * 2; b.y += oy * 2 }
          else { a.x -= ox * 2; a.y -= oy * 2 }
        }
      }
    }
  }

  if (dragging) alpha = Math.max(alpha, 0.3)
  else alpha += (0 - alpha) * ALPHA_DECAY
  version.value++

  if (alpha < ALPHA_MIN && !dragging) {
    running = false
    if (pendingFit) { pendingFit = false; fitView() }
  } else {
    raf = requestAnimationFrame(tick)
  }
}
onBeforeUnmount(() => { alive = false; if (raf) cancelAnimationFrame(raf) })

// Reactive views (recomputed each tick via `version`).
const nodesView = computed<GNode[]>(() => { version.value; return simNodes.slice() })
const edges = computed(() => {
  version.value
  const srcById = new Map(sources.value.map(s => [s.id, s]))
  const out: { x1: number; y1: number; x2: number; y2: number; color: string }[] = []
  for (const nd of simNodes) {
    if (nd.isCenter) continue
    for (const sid of nd.sourceIds) {
      const s = srcById.get(sid); if (!s) continue
      const hub = nodeIndex.get(s.hubKey); if (!hub) continue
      out.push({ x1: hub.x, y1: hub.y, x2: nd.x, y2: nd.y, color: s.color })
    }
  }
  return out
})

function nodeFill(n: GNode): string {
  const lib = !!n.ref?.library_slug
  const cols = n.sourceIds.map(id => {
    const c = sourceColor(id)
    return lib ? c : `color-mix(in srgb, ${c} 22%, var(--bg-primary))`
  })
  if (cols.length <= 1) return cols[0] ?? '#cbd5e1'
  const seg = 100 / cols.length
  const stops = cols.map((c, i) => `${c} ${i * seg}% ${(i + 1) * seg}%`).join(', ')
  return `conic-gradient(${stops})`
}

// ── Selection / text ─────────────────────────────────────────────────────────
const selected = computed(() => (selectedKey.value ? nodeIndex.get(selectedKey.value) ?? null : null))
function subLine(r: CitationRef) {
  const first = r.authors?.[0] ?? ''
  const etal = (r.authors?.length ?? 0) > 1 ? ' et al.' : ''
  return [first + etal, r.venue, r.year?.toString()].filter(Boolean).join(' · ')
}
function venueYear(r: CitationRef) { return [r.venue, r.year?.toString()].filter(Boolean).join(' · ') }
function nodeTooltip(r: CitationRef) {
  const parts = [r.title]
  const vy = venueYear(r); if (vy) parts.push(vy)
  if (r.cite_count != null) parts.push(`${r.cite_count} citations`)
  return parts.join('\n')
}
function citationText(r: CitationRef) {
  return [r.title, subLine(r), r.cite_count != null ? `${r.cite_count} citations` : ''].filter(Boolean).join(' · ')
}
function openSelected() { if (selected.value?.ref?.library_slug) emit('open', selected.value.ref.library_slug) }
async function copySelected() {
  if (!selected.value?.ref) return
  try {
    await navigator.clipboard.writeText(citationText(selected.value.ref))
    copied.value = true
    setTimeout(() => { copied.value = false }, 1500)
  } catch { /* text is still selectable */ }
}
function shortTitle(s: string) { return s.length > 22 ? s.slice(0, 21) + '…' : s }

// ── Node drag + click ────────────────────────────────────────────────────────
let dragNode: GNode | null = null
let dragMoved = false
let dragStart = { x: 0, y: 0 }
let dragOrigin = { x: 0, y: 0 }

function onNodeDown(e: MouseEvent, n: GNode) {
  if (n.isCenter) return
  e.stopPropagation()
  dragNode = n
  dragMoved = false
  dragStart = { x: e.clientX, y: e.clientY }
  dragOrigin = { x: n.x, y: n.y }
  window.addEventListener('mousemove', onNodeMove)
  window.addEventListener('mouseup', onNodeUp)
}
function onNodeMove(e: MouseEvent) {
  if (!dragNode) return
  const dx = e.clientX - dragStart.x, dy = e.clientY - dragStart.y
  if (!dragMoved && Math.hypot(dx, dy) < 4) return
  dragMoved = true
  dragging = true
  dragNode.fx = dragOrigin.x + dx / view.k
  dragNode.fy = dragOrigin.y + dy / view.k
  dragNode.x = dragNode.fx; dragNode.y = dragNode.fy
  reheat(0.3)
}
function onNodeUp() {
  window.removeEventListener('mousemove', onNodeMove)
  window.removeEventListener('mouseup', onNodeUp)
  const n = dragNode
  dragNode = null
  dragging = false
  if (!n) return
  if (!dragMoved) {
    // A click (not a drag): select, and expand if it's an in-library node.
    selectedKey.value = n.key
    if (n.ref?.library_slug) expand(n)
  } else {
    // Release the pin: the node stays where it was dropped but rejoins the
    // simulation, so it still gets pushed when other nodes collide with it
    // (instead of being nailed in place).
    n.fx = null
    n.fy = null
    reheat(0.3)
  }
}
function onNodeDbl(n: GNode) { if (n.ref?.library_slug) emit('open', n.ref.library_slug) }

// ── Viewport sizing + pan/zoom ───────────────────────────────────────────────
const vpEl = ref<HTMLElement | null>(null)
const W = ref(600)
const H = ref(460)
const CX = computed(() => W.value / 2)
const CY = computed(() => H.value / 2)
let resizeObs: ResizeObserver | null = null
function measureViewport() {
  const el = vpEl.value
  if (!el) return
  W.value = el.clientWidth || 600
  H.value = el.clientHeight || 460
}
onMounted(() => {
  measureViewport()
  if (vpEl.value && typeof ResizeObserver !== 'undefined') {
    resizeObs = new ResizeObserver(measureViewport)
    resizeObs.observe(vpEl.value)
  }
})
onBeforeUnmount(() => resizeObs?.disconnect())
watch([W, H], () => fitView())

const view = reactive({ x: 0, y: 0, k: 1 })
function fitView() {
  const ns = simNodes
  const halfW = 80
  let minX = -CENTER_D / 2, minY = -CENTER_D / 2, maxX = CENTER_D / 2, maxY = CENTER_D / 2
  for (const nd of ns) {
    const r = radiusOf(nd)
    minX = Math.min(minX, nd.x - Math.max(r, halfW))
    maxX = Math.max(maxX, nd.x + Math.max(r, halfW))
    minY = Math.min(minY, nd.y - r)
    maxY = Math.max(maxY, nd.y + r + 40)
  }
  const pad = 24
  const raw = Math.min(W.value / (maxX - minX + pad * 2), H.value / (maxY - minY + pad * 2), 1.1)
  const fit = raw > 0 && isFinite(raw) ? raw : 1
  if (fit < MIN_FIT_K) { view.k = MIN_FIT_K; view.x = 0; view.y = 0 }
  else { view.k = fit; view.x = -((minX + maxX) / 2) * fit; view.y = -((minY + maxY) / 2) * fit }
}
function resetView() { rebuildBase() }

let panning = false
let panStart = { x: 0, y: 0 }
let viewStart = { x: 0, y: 0 }
function onBgDown(e: MouseEvent) {
  selectedKey.value = null
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
  view.k = Math.min(2.6, Math.max(0.25, view.k * (e.deltaY < 0 ? 1.12 : 0.89)))
}
onBeforeUnmount(onBgUp)

const edgeTransform = computed(() => `translate(${CX.value + view.x}, ${CY.value + view.y}) scale(${view.k})`)
const nodesTransform = computed(() => `translate(${CX.value + view.x}px, ${CY.value + view.y}px) scale(${view.k})`)
</script>

<template>
  <div class="cite-graph">
    <div
      ref="vpEl"
      class="cg-viewport"
      :style="{ backgroundPosition: `${view.x}px ${view.y}px` }"
      @mousedown="onBgDown"
      @wheel="onWheel"
    >
      <svg class="cg-edges" :width="W" :height="H" :viewBox="`0 0 ${W} ${H}`">
        <g :transform="edgeTransform">
          <line
            v-for="(e, i) in edges"
            :key="i"
            class="cg-link"
            :x1="e.x1" :y1="e.y1" :x2="e.x2" :y2="e.y2"
            :style="{ stroke: e.color }"
          />
        </g>
      </svg>

      <div class="cg-nodes" :style="{ transform: nodesTransform }">
        <div
          v-for="n in nodesView"
          :key="n.key"
          class="cg-node"
          :class="{ center: n.isCenter }"
          :style="{ transform: `translate(${n.x}px, ${n.y}px) translate(-50%, -50%)` }"
          @mousedown="onNodeDown($event, n)"
          @click.stop
          @dblclick.stop="onNodeDbl(n)"
        >
          <div
            class="cg-bubble"
            :class="{ center: n.isCenter, lib: !n.isCenter && !!n.ref?.library_slug, selected: selectedKey === n.key, expanding: expandingId && expandingId === n.ref?.library_slug }"
            :style="{ width: `${n.isCenter ? CENTER_D : diamOf(n.ref?.cite_count)}px`, height: `${n.isCenter ? CENTER_D : diamOf(n.ref?.cite_count)}px`, background: n.isCenter ? undefined : nodeFill(n) }"
            :title="n.isCenter ? centerTitle : nodeTooltip(n.ref!)"
          >
            <span v-if="!n.isCenter && n.ref?.cite_count != null && diamOf(n.ref?.cite_count) >= 46" class="cg-bubble-num">{{ n.ref.cite_count }}</span>
          </div>
          <div class="cg-label" :class="{ center: n.isCenter, sel: selectedKey === n.key }">
            <span class="cg-label-title">{{ n.isCenter ? centerTitle : n.ref?.title }}</span>
            <span v-if="!n.isCenter && n.ref && venueYear(n.ref)" class="cg-label-sub">{{ venueYear(n.ref) }}</span>
          </div>
        </div>
      </div>

      <div v-if="nodesView.length <= 1" class="cg-empty">{{ t('citeGraph.empty') }}</div>
    </div>

    <div class="cg-footer">
      <div class="cg-legend">
        <span class="cg-leg"><i class="dot lib-dot" /> {{ t('citeGraph.legendInLib') }}</span>
        <span class="cg-leg"><i class="dot pale-dot" /> {{ t('citeGraph.legendNotInLib') }}</span>
        <span v-for="s in sources" :key="s.id" class="cg-leg">
          <i class="dot" :style="{ background: s.color }" />
          {{ s.id === '__root__' ? t('citeGraph.legendThis') : shortTitle(s.title) }}
        </span>
        <span class="cg-leg cg-size-hint">{{ t('citeGraph.legendSize') }}</span>
        <span class="cg-leg cg-size-hint">{{ t('citeGraph.legendExpand') }}</span>
        <span v-if="hiddenCount" class="cg-leg cg-more">{{ t('citeGraph.moreHidden', { n: hiddenCount }) }}</span>
      </div>
      <div class="cg-foot-right">
        <div v-if="selected?.ref" class="cg-sel">
          <div class="cg-sel-title">{{ selected.ref.title }}</div>
          <div class="cg-sel-sub">
            {{ subLine(selected.ref) }}
            <span v-if="selected.ref.cite_count != null"> · {{ selected.ref.cite_count.toLocaleString() }} citations</span>
          </div>
        </div>
        <button v-if="selected?.ref" class="cg-btn" @click="copySelected">
          {{ copied ? t('citeGraph.copied') : t('citeGraph.copy') }}
        </button>
        <button v-if="selected?.ref?.library_slug" class="cg-btn accent" @click="openSelected">{{ t('citeGraph.open') }}</button>
        <button class="cg-reset" @click="resetView">{{ t('citeGraph.resetView') }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cite-graph { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.cg-viewport {
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
.cg-viewport:active { cursor: grabbing; }

.cg-edges { position: absolute; inset: 0; pointer-events: none; }
.cg-link { stroke-width: 1.3; opacity: 0.4; }

.cg-nodes { position: absolute; inset: 0; transform-origin: 0 0; }

.cg-node { position: absolute; left: 0; top: 0; cursor: grab; }
.cg-node:active { cursor: grabbing; }
.cg-node.center { cursor: default; }

/* Not-in-library: pale fill + thin light border + muted number. */
.cg-bubble {
  border-radius: 50%;
  border: 1.5px solid var(--border-default);
  color: var(--text-secondary);
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.05);
  transition: box-shadow 0.15s, transform 0.1s;
  animation: cg-in 0.3s ease;    /* opacity-only fade so physics motion isn't fought */
}
.cg-bubble.lib {
  border: 2.5px solid var(--text-primary);
  color: #fff;
  box-shadow: 0 2px 7px rgba(0, 0, 0, 0.16);
}
.cg-node:hover .cg-bubble { transform: scale(1.07); }
.cg-bubble.center {
  background: var(--accent);
  border: 2.5px solid var(--accent);
  color: #fff;
  box-shadow: 0 0 0 5px color-mix(in srgb, var(--accent) 22%, transparent);
}
.cg-bubble.selected { box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 40%, transparent), 0 2px 7px rgba(0,0,0,0.16); }
.cg-bubble.expanding { animation: cg-pulse 0.9s ease-in-out infinite; }
.cg-bubble-num { font-size: 11px; font-weight: 700; font-variant-numeric: tabular-nums; color: inherit; }
.cg-bubble.lib .cg-bubble-num { text-shadow: 0 1px 2px rgba(0, 0, 0, 0.35); }
@keyframes cg-in { from { opacity: 0; } }
@keyframes cg-pulse { 0%,100% { box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 30%, transparent); } 50% { box-shadow: 0 0 0 9px color-mix(in srgb, var(--accent) 8%, transparent); } }

.cg-label {
  position: absolute;
  top: 100%;
  left: 50%;
  transform: translateX(-50%);
  margin-top: 4px;
  width: 150px;
  text-align: center;
  pointer-events: none;
}
.cg-label-title {
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  font-size: 12px;
  line-height: 1.32;
  font-weight: 500;
  color: var(--text-primary);
}
.cg-label-sub {
  display: block;
  margin-top: 2px;
  font-size: 10.5px;
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cg-label.center { width: 168px; }
.cg-label.center .cg-label-title { font-weight: 700; }
.cg-label.sel .cg-label-title { color: var(--accent); font-weight: 700; }

.cg-empty {
  position: absolute;
  top: 50%; left: 0; right: 0;
  transform: translateY(-50%);
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  pointer-events: none;
}

.cg-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 8px;
  min-height: 24px;
}
.cg-legend { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.cg-leg { display: inline-flex; align-items: center; gap: 4px; font-size: var(--font-size-xs); color: var(--text-tertiary); }
.cg-leg .dot { width: 10px; height: 10px; border-radius: 50%; }
.cg-leg .lib-dot { background: var(--accent); border: 2px solid var(--text-primary); }
.cg-leg .pale-dot { background: color-mix(in srgb, var(--accent) 22%, var(--bg-primary)); border: 1.5px solid var(--border-default); }
.cg-more { color: var(--text-secondary); }

.cg-foot-right { display: flex; align-items: center; gap: 8px; min-width: 0; flex: 1; justify-content: flex-end; }
.cg-sel { min-width: 0; max-width: 480px; text-align: right; user-select: text; }
.cg-sel-title {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  font-weight: 500;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  line-height: 1.3;
  user-select: text;
}
.cg-sel-sub {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  user-select: text;
}
.cg-btn {
  flex-shrink: 0;
  border: 1px solid var(--border-default);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  padding: 3px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.cg-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.cg-btn.accent { border-color: var(--accent); color: var(--accent); }
.cg-btn.accent:hover { background: color-mix(in srgb, var(--accent) 12%, transparent); }
.cg-reset {
  flex-shrink: 0;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
}
.cg-reset:hover { background: var(--bg-hover); color: var(--text-primary); }
</style>
