<script setup lang="ts">
import { ref, shallowRef, computed, onMounted, onUnmounted, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { emitTo } from '@tauri-apps/api/event'
import type { EmbeddingMapData, EmbeddingMapPaper } from '../types'

const { t } = useI18n()

// ── Data state ────────────────────────────────────────────────────────────────
const loading = ref(true)
const error = ref('')
// shallowRef: chunk arrays can be large — keep them out of deep reactivity
// so the per-frame canvas draw doesn't go through proxies
const data = shallowRef<EmbeddingMapData | null>(null)

// ── UI state (threshold & chunk toggle persist across sessions) ──────────────
const THRESHOLD_KEY = 'argus-embed-map-threshold'
const SHOW_CHUNKS_KEY = 'argus-embed-map-show-chunks'
const THRESHOLD_MIN = 0.25
const THRESHOLD_MAX = 0.95

function loadThreshold(): number {
  try {
    const raw = Number(localStorage.getItem(THRESHOLD_KEY))
    if (Number.isFinite(raw) && raw > 0) {
      return Math.min(THRESHOLD_MAX, Math.max(THRESHOLD_MIN, raw))
    }
  } catch { /* storage unavailable */ }
  return 0.55
}

function loadShowChunks(): boolean {
  try { return localStorage.getItem(SHOW_CHUNKS_KEY) !== 'false' } catch { return true }
}

const simThreshold = ref(loadThreshold())
const showChunks = ref(loadShowChunks())
// Which embedding model's vectors are shown. Empty = let the backend pick the
// current/default model; it echoes the resolved model back in the response.
const selectedModel = ref('')

// Title search: highlights matching papers and dims the rest.
const searchQuery = ref('')
const searchMatches = computed<Set<number> | null>(() => {
  const q = searchQuery.value.trim().toLowerCase()
  const d = data.value
  if (!q || !d) return null
  const s = new Set<number>()
  d.papers.forEach((p, i) => { if (p.title.toLowerCase().includes(q)) s.add(i) })
  return s
})

function onThresholdInput(e: Event) {
  const el = e.target as HTMLInputElement
  const v = parseFloat(el.value)
  if (Number.isFinite(v)) {
    simThreshold.value = Math.min(THRESHOLD_MAX, Math.max(THRESHOLD_MIN, v))
  }
  el.value = simThreshold.value.toFixed(2)
}

function blurTarget(e: Event) {
  (e.target as HTMLInputElement).blur()
}
const hoverInfo = ref<{ kind: 'paper' | 'chunk'; title: string; sub: string; x: number; y: number } | null>(null)
const selectedIdx = ref<number | null>(null)

// ── Communities & degrees (relative to the current threshold) ────────────────
interface GraphInfo {
  /** Degree of each paper under the current link threshold */
  degrees: Uint16Array
  /** Community rank per paper, sorted by community size; -1 = isolated/singleton */
  compOf: Int32Array
  /** Number of communities with ≥ 2 members */
  clusterCount: number
}

/**
 * Louvain modularity clustering (weighted). Unlike plain connected components,
 * two groups joined by only a few weak links still count as separate clusters —
 * a community sticks together only when its internal edge weight is high
 * relative to what random chance would predict. Deterministic: nodes are
 * visited in index order.
 */
function louvain(n: number, edges: { a: number; b: number; w: number }[]): Int32Array {
  // mapping[v] = community of original node v (refined level by level)
  let mapping = new Int32Array(n)
  for (let i = 0; i < n; i++) mapping[i] = i
  let size = n
  let curEdges = edges

  for (let level = 0; level < 10; level++) {
    const adj: Map<number, number>[] = Array.from({ length: size }, () => new Map())
    const kDeg = new Float64Array(size)
    let m2 = 0 // 2m (total weight × 2)
    for (const e of curEdges) {
      if (e.a === e.b) {
        kDeg[e.a] += 2 * e.w
        m2 += 2 * e.w
        continue
      }
      adj[e.a].set(e.b, (adj[e.a].get(e.b) ?? 0) + e.w)
      adj[e.b].set(e.a, (adj[e.b].get(e.a) ?? 0) + e.w)
      kDeg[e.a] += e.w
      kDeg[e.b] += e.w
      m2 += 2 * e.w
    }
    if (m2 === 0) break

    const comm = new Int32Array(size)
    for (let i = 0; i < size; i++) comm[i] = i
    const sigmaTot = Float64Array.from(kDeg)

    // Local moving phase
    let improvedAny = false
    let moved = true
    let guard = 0
    while (moved && guard++ < 60) {
      moved = false
      for (let i = 0; i < size; i++) {
        const ci = comm[i]
        const wTo = new Map<number, number>()
        for (const [j, w] of adj[i]) {
          wTo.set(comm[j], (wTo.get(comm[j]) ?? 0) + w)
        }
        sigmaTot[ci] -= kDeg[i]
        let bestC = ci
        let bestGain = (wTo.get(ci) ?? 0) - (sigmaTot[ci] * kDeg[i]) / m2
        for (const [cand, w] of wTo) {
          if (cand === ci) continue
          const gain = w - (sigmaTot[cand] * kDeg[i]) / m2
          if (gain > bestGain + 1e-12) {
            bestGain = gain
            bestC = cand
          }
        }
        sigmaTot[bestC] += kDeg[i]
        if (bestC !== ci) {
          comm[i] = bestC
          moved = true
          improvedAny = true
        }
      }
    }
    if (!improvedAny) break

    // Renumber communities compactly
    const renum = new Map<number, number>()
    for (let i = 0; i < size; i++) {
      if (!renum.has(comm[i])) renum.set(comm[i], renum.size)
    }
    const newSize = renum.size
    const newMapping = new Int32Array(n)
    for (let v = 0; v < n; v++) {
      newMapping[v] = renum.get(comm[mapping[v]])!
    }
    mapping = newMapping
    if (newSize === size) break

    // Aggregation phase: communities become super-nodes
    const agg = new Map<number, number>()
    for (const e of curEdges) {
      const ca = renum.get(comm[e.a])!
      const cb = renum.get(comm[e.b])!
      const a = Math.min(ca, cb)
      const b = Math.max(ca, cb)
      const key = a * newSize + b
      agg.set(key, (agg.get(key) ?? 0) + e.w)
    }
    curEdges = [...agg.entries()].map(([key, w]) => ({
      a: Math.floor(key / newSize),
      b: key % newSize,
      w,
    }))
    size = newSize
  }

  return mapping
}

const graphInfo = computed<GraphInfo | null>(() => {
  const d = data.value
  if (!d) return null
  const n = d.papers.length
  const thr = simThreshold.value

  const degrees = new Uint16Array(n)
  const active: { a: number; b: number; w: number }[] = []
  for (const e of d.edges) {
    if (e.sim < thr) continue
    degrees[e.a]++
    degrees[e.b]++
    active.push({ a: e.a, b: e.b, w: e.sim })
  }

  const commOf = louvain(n, active)

  // Rank communities by size (largest first) so hues stay visually stable;
  // singletons (incl. isolated nodes) get no color rank.
  const sizeOfComm = new Map<number, number>()
  for (let i = 0; i < n; i++) {
    sizeOfComm.set(commOf[i], (sizeOfComm.get(commOf[i]) ?? 0) + 1)
  }
  const rankOfComm = new Map<number, number>()
  let rank = 0
  for (const [comm, size] of [...sizeOfComm.entries()].sort((a, b) => b[1] - a[1])) {
    if (size >= 2) rankOfComm.set(comm, rank++)
  }
  const compOf = new Int32Array(n)
  for (let i = 0; i < n; i++) {
    compOf[i] = rankOfComm.get(commOf[i]) ?? -1
  }
  return { degrees, compOf, clusterCount: rank }
})

const stats = computed(() => {
  const d = data.value
  if (!d) return ''
  const parts = [
    t('embedMap.stats', { papers: d.papers.length, chunks: d.chunks.length, dim: d.dimension }),
  ]
  const g = graphInfo.value
  if (g && g.clusterCount > 0) parts.push(t('embedMap.clusters', { n: g.clusterCount }))
  if (d.embedding_model) parts.push(d.embedding_model)
  return parts.join(' · ')
})

const selectedPaper = computed(() =>
  data.value && selectedIdx.value != null ? data.value.papers[selectedIdx.value] : null
)

interface NeighborItem { idx: number; title: string; sim: number }
const selectedNeighbors = computed<NeighborItem[]>(() => {
  const d = data.value
  const sel = selectedIdx.value
  if (!d || sel == null) return []
  return d.edges
    .filter(e => e.a === sel || e.b === sel)
    .map(e => {
      const other = e.a === sel ? e.b : e.a
      return { idx: other, title: d.papers[other].title, sim: e.sim }
    })
    .sort((a, b) => b.sim - a.sim)
    .slice(0, 10)
})

// ── Layout simulation ─────────────────────────────────────────────────────────
// World scale: PCA coords are z-scored (≈ ±3); spread them out.
const WORLD_SCALE = 170

let px: Float64Array = new Float64Array(0) // node positions
let py: Float64Array = new Float64Array(0)
let vx: Float64Array = new Float64Array(0)
let vy: Float64Array = new Float64Array(0)
let ax: Float64Array = new Float64Array(0) // PCA anchors
let ay: Float64Array = new Float64Array(0)
let alpha = 0

// springs used by the simulation (fixed, independent of the display slider)
let simEdges: { a: number; b: number; sim: number }[] = []

function initLayout() {
  const d = data.value
  if (!d) return
  const n = d.papers.length
  px = new Float64Array(n)
  py = new Float64Array(n)
  vx = new Float64Array(n)
  vy = new Float64Array(n)
  ax = new Float64Array(n)
  ay = new Float64Array(n)
  for (let i = 0; i < n; i++) {
    ax[i] = d.papers[i].x * WORLD_SCALE
    ay[i] = d.papers[i].y * WORLD_SCALE
    px[i] = ax[i]
    py[i] = ay[i]
  }
  simEdges = d.edges.filter(e => e.sim >= 0.35)
  alpha = 1
  fitView()
  scheduleFrame()
}

function tickSim() {
  const d = data.value
  if (!d || alpha <= 0.02) { alpha = 0; return }
  const n = d.papers.length

  // Pairwise repulsion (n is personal-library scale; O(n²) is fine)
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      let dx = px[i] - px[j]
      let dy = py[i] - py[j]
      const d2 = dx * dx + dy * dy + 80
      const f = (2400 / d2) * alpha
      const inv = 1 / Math.sqrt(d2)
      dx *= inv; dy *= inv
      vx[i] += dx * f; vy[i] += dy * f
      vx[j] -= dx * f; vy[j] -= dy * f
    }
  }

  // Springs along similarity edges
  for (const e of simEdges) {
    const dx = px[e.b] - px[e.a]
    const dy = py[e.b] - py[e.a]
    const dist = Math.sqrt(dx * dx + dy * dy) + 1e-6
    const rest = 70 + (1 - e.sim) * 260
    const f = 0.05 * e.sim * (dist - rest) * alpha
    const ux = dx / dist, uy = dy / dist
    vx[e.a] += ux * f; vy[e.a] += uy * f
    vx[e.b] -= ux * f; vy[e.b] -= uy * f
  }

  // Weak anchor back to the PCA position keeps the global semantic layout
  for (let i = 0; i < n; i++) {
    vx[i] += (ax[i] - px[i]) * 0.012 * alpha
    vy[i] += (ay[i] - py[i]) * 0.012 * alpha
    if (i === dragNode) continue
    px[i] += vx[i]
    py[i] += vy[i]
    vx[i] *= 0.82
    vy[i] *= 0.82
  }

  alpha *= 0.975
}

// ── Canvas / view transform ───────────────────────────────────────────────────
const canvasRef = ref<HTMLCanvasElement | null>(null)
const wrapRef = ref<HTMLDivElement | null>(null)
let ctx: CanvasRenderingContext2D | null = null
let cssW = 0
let cssH = 0
let scale = 1
let tx = 0
let ty = 0
let rafId = 0
let frameQueued = false

function fitView() {
  const d = data.value
  if (!d || !d.papers.length || !cssW) return
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity
  for (let i = 0; i < px.length; i++) {
    minX = Math.min(minX, px[i]); maxX = Math.max(maxX, px[i])
    minY = Math.min(minY, py[i]); maxY = Math.max(maxY, py[i])
  }
  const spanX = Math.max(maxX - minX, 100)
  const spanY = Math.max(maxY - minY, 100)
  scale = Math.min(cssW / (spanX * 1.25), cssH / (spanY * 1.25), 3)
  tx = cssW / 2 - ((minX + maxX) / 2) * scale
  ty = cssH / 2 - ((minY + maxY) / 2) * scale
}

// Pan/zoom so the given nodes fit comfortably in view (used by search).
function fitNodes(indices: number[]) {
  if (!indices.length || !cssW) return
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity
  for (const i of indices) {
    minX = Math.min(minX, px[i]); maxX = Math.max(maxX, px[i])
    minY = Math.min(minY, py[i]); maxY = Math.max(maxY, py[i])
  }
  const spanX = Math.max(maxX - minX, 120)
  const spanY = Math.max(maxY - minY, 120)
  scale = Math.min(cssW / (spanX * 1.6), cssH / (spanY * 1.6), 2.4)
  tx = cssW / 2 - ((minX + maxX) / 2) * scale
  ty = cssH / 2 - ((minY + maxY) / 2) * scale
  scheduleFrame()
}

function toScreen(wx: number, wy: number): [number, number] {
  return [wx * scale + tx, wy * scale + ty]
}

function toWorld(sx: number, sy: number): [number, number] {
  return [(sx - tx) / scale, (sy - ty) / scale]
}

// ── Theme colors ──────────────────────────────────────────────────────────────
let themeDark = false
let colBg = '#ffffff'
let colText = '#1a1a1a'
let colTextDim = '#888888'
let colEdge = '#94a3b8'

function readTheme() {
  const styles = getComputedStyle(document.documentElement)
  colBg = styles.getPropertyValue('--bg-primary').trim() || '#ffffff'
  colText = styles.getPropertyValue('--text-primary').trim() || '#1a1a1a'
  colTextDim = styles.getPropertyValue('--text-tertiary').trim() || '#888888'
  // crude luminance check on hex colors
  const m = /^#?([0-9a-f]{6})$/i.exec(colBg)
  if (m) {
    const v = parseInt(m[1], 16)
    const lum = (((v >> 16) & 255) * 0.299 + ((v >> 8) & 255) * 0.587 + (v & 255) * 0.114) / 255
    themeDark = lum < 0.5
  }
  colEdge = themeDark ? '#64748b' : '#94a3b8'
}

// Cluster color: hue by component rank (golden angle keeps neighbors distinct);
// isolated nodes are neutral gray.
function paperColorOf(g: GraphInfo | null, i: number, alphaV = 1) {
  const comp = g ? g.compOf[i] : -1
  if (comp < 0) {
    return `hsla(220, 9%, ${themeDark ? 58 : 55}%, ${alphaV})`
  }
  const hue = (comp * 137.508) % 360
  return `hsla(${hue}, 62%, ${themeDark ? 64 : 46}%, ${alphaV})`
}

// Template-facing variant (panel dot, neighbor dots)
function paperColor(i: number, alphaV = 1) {
  return paperColorOf(graphInfo.value, i, alphaV)
}

// ── Render ────────────────────────────────────────────────────────────────────
// Node size scales with its degree under the current threshold
function nodeRadiusOf(g: GraphInfo | null, i: number) {
  const deg = g?.degrees[i] ?? 0
  return Math.min(5 + Math.sqrt(deg) * 2.8, 19)
}

function scheduleFrame() {
  if (frameQueued) return
  frameQueued = true
  rafId = requestAnimationFrame(frame)
}

function frame() {
  frameQueued = false
  if (alpha > 0.02) {
    tickSim()
    scheduleFrame()
  }
  draw()
}

function draw() {
  const c = ctx
  const d = data.value
  if (!c || !d) return
  c.clearRect(0, 0, cssW, cssH)

  const sel = selectedIdx.value
  const hoverIdx = hoverPaperIdx
  const focusSet = focusedSet()
  const g = graphInfo.value

  // ── Edges: intra-cluster tinted by cluster color, inter-cluster neutral ──
  for (const e of d.edges) {
    if (e.sim < simThreshold.value) continue
    const [x1, y1] = toScreen(px[e.a], py[e.a])
    const [x2, y2] = toScreen(px[e.b], py[e.b])
    const isFocus = sel != null && (e.a === sel || e.b === sel)
    const strength = Math.min(1, (e.sim - simThreshold.value) / Math.max(1 - simThreshold.value, 0.05))
    let lineAlpha = 0.14 + strength * 0.4
    if (sel != null) lineAlpha = isFocus ? Math.max(lineAlpha, 0.6) : lineAlpha * 0.25
    const sameCluster = g != null && g.compOf[e.a] >= 0 && g.compOf[e.a] === g.compOf[e.b]
    c.strokeStyle = sameCluster ? paperColorOf(g, e.a, 1) : colEdge
    c.globalAlpha = sameCluster ? lineAlpha : lineAlpha * 0.55
    c.lineWidth = isFocus ? 1.8 : 0.8 + strength * 1.2
    c.beginPath()
    c.moveTo(x1, y1)
    c.lineTo(x2, y2)
    c.stroke()
  }
  c.globalAlpha = 1

  // ── Chunk dots (offset relative to their paper's current position) ──
  if (showChunks.value) {
    const dotR = Math.max(1.4, Math.min(2.6, 1.2 * scale))
    for (let i = 0; i < d.chunks.length; i++) {
      const ch = d.chunks[i]
      const p = ch.paper
      const wx = px[p] + (ch.x - d.papers[p].x) * WORLD_SCALE * 0.55
      const wy = py[p] + (ch.y - d.papers[p].y) * WORLD_SCALE * 0.55
      const [x, y] = toScreen(wx, wy)
      if (x < -8 || y < -8 || x > cssW + 8 || y > cssH + 8) continue
      const dimmed = focusSet != null && !focusSet.has(p)
      c.fillStyle = paperColorOf(g, p, dimmed ? 0.06 : 0.28)
      c.beginPath()
      c.arc(x, y, dotR, 0, Math.PI * 2)
      c.fill()
    }
  }

  // ── Paper nodes (radius = degree under current threshold) ──
  for (let i = 0; i < d.papers.length; i++) {
    const [x, y] = toScreen(px[i], py[i])
    const r = nodeRadiusOf(g, i)
    if (x < -r - 20 || y < -r - 20 || x > cssW + r + 20 || y > cssH + r + 20) continue
    const dimmed = focusSet != null && !focusSet.has(i)
    const fill = paperColorOf(g, i, dimmed ? 0.18 : 0.9)
    const ring = paperColorOf(g, i, dimmed ? 0.35 : 1)
    // Reading status encodes fill: read = solid, reading = half-filled, unread = hollow.
    const status = d.papers[i].reading_status
    if (status === 'read') {
      c.fillStyle = fill
      c.beginPath()
      c.arc(x, y, r, 0, Math.PI * 2)
      c.fill()
    } else {
      // Hollow base so edges/chunks don't bleed through the ring.
      c.fillStyle = colBg
      c.beginPath()
      c.arc(x, y, r, 0, Math.PI * 2)
      c.fill()
      if (status === 'reading') {
        // Fill the left half only.
        c.fillStyle = fill
        c.beginPath()
        c.arc(x, y, r, Math.PI / 2, Math.PI * 1.5)
        c.closePath()
        c.fill()
      }
      c.strokeStyle = ring
      c.lineWidth = Math.max(1.4, r * 0.2)
      c.beginPath()
      c.arc(x, y, r, 0, Math.PI * 2)
      c.stroke()
    }
    const matched = searchMatches.value?.has(i) ?? false
    if (i === sel || i === hoverIdx || matched) {
      c.strokeStyle = matched ? '#f59e0b' : paperColorOf(g, i, 1)
      c.lineWidth = matched ? 2.8 : 2
      c.beginPath()
      c.arc(x, y, r + 3.5, 0, Math.PI * 2)
      c.stroke()
    }
  }

  // ── Labels ──
  const showAll = d.papers.length <= 80 && scale > 0.35
  c.font = '11px -apple-system, "PingFang SC", sans-serif'
  c.textBaseline = 'top'
  for (let i = 0; i < d.papers.length; i++) {
    const isFocus = i === sel || i === hoverIdx || (focusSet?.has(i) ?? false)
    if (!showAll && !isFocus) continue
    const dimmed = focusSet != null && !focusSet.has(i)
    if (dimmed) continue
    const [x, y] = toScreen(px[i], py[i])
    if (x < -150 || y < -30 || x > cssW + 150 || y > cssH + 30) continue
    const r = nodeRadiusOf(g, i)
    let label = d.papers[i].title
    if (label.length > 32) label = label.slice(0, 31) + '…'
    const w = c.measureText(label).width
    const lx = x - w / 2
    const ly = y + r + 4
    c.fillStyle = colBg
    c.globalAlpha = 0.72
    c.fillRect(lx - 2, ly - 1, w + 4, 14)
    c.globalAlpha = 1
    c.fillStyle = isFocus ? colText : colTextDim
    c.fillText(label, lx, ly)
  }
}

function focusedSet(): Set<number> | null {
  const d = data.value
  if (!d) return null
  // An active search takes over the dim/highlight baseline.
  if (searchMatches.value) return searchMatches.value
  const sel = selectedIdx.value
  if (sel == null) return null
  const s = new Set<number>([sel])
  for (const e of d.edges) {
    if (e.sim < simThreshold.value) continue
    if (e.a === sel) s.add(e.b)
    if (e.b === sel) s.add(e.a)
  }
  return s
}

// ── Hit testing ───────────────────────────────────────────────────────────────
let hoverPaperIdx: number | null = null

function hitPaper(sx: number, sy: number): number | null {
  const d = data.value
  if (!d) return null
  const g = graphInfo.value
  let best: number | null = null
  let bestDist = Infinity
  for (let i = 0; i < d.papers.length; i++) {
    const [x, y] = toScreen(px[i], py[i])
    const r = nodeRadiusOf(g, i) + 4
    const dist = Math.hypot(sx - x, sy - y)
    if (dist <= r && dist < bestDist) {
      best = i
      bestDist = dist
    }
  }
  return best
}

function hitChunk(sx: number, sy: number): number | null {
  const d = data.value
  if (!d || !showChunks.value) return null
  let best: number | null = null
  // Match the on-screen dot radius (see render) so hit-testing scales with zoom
  // instead of using a fixed 7px that felt unresponsive when zoomed in/out.
  const dotR = Math.max(1.4, Math.min(2.6, 1.2 * scale))
  let bestDist = Math.max(6, dotR + 3)
  for (let i = 0; i < d.chunks.length; i++) {
    const ch = d.chunks[i]
    const p = ch.paper
    const wx = px[p] + (ch.x - d.papers[p].x) * WORLD_SCALE * 0.55
    const wy = py[p] + (ch.y - d.papers[p].y) * WORLD_SCALE * 0.55
    const [x, y] = toScreen(wx, wy)
    const dist = Math.hypot(sx - x, sy - y)
    if (dist < bestDist) {
      best = i
      bestDist = dist
    }
  }
  return best
}

// ── Pointer interaction ───────────────────────────────────────────────────────
let dragging = false
let dragNode: number | null = null
let dragMoved = false
let lastMx = 0
let lastMy = 0

function onPointerDown(e: PointerEvent) {
  const rect = canvasRef.value!.getBoundingClientRect()
  const mx = e.clientX - rect.left
  const my = e.clientY - rect.top
  lastMx = mx
  lastMy = my
  dragMoved = false
  const hit = hitPaper(mx, my)
  if (hit != null) {
    dragNode = hit
  } else {
    dragging = true
  }
  canvasRef.value!.setPointerCapture(e.pointerId)
}

function onPointerMove(e: PointerEvent) {
  const rect = canvasRef.value!.getBoundingClientRect()
  const mx = e.clientX - rect.left
  const my = e.clientY - rect.top

  if (dragNode != null) {
    const [wx, wy] = toWorld(mx, my)
    px[dragNode] = wx
    py[dragNode] = wy
    vx[dragNode] = 0
    vy[dragNode] = 0
    if (Math.hypot(mx - lastMx, my - lastMy) > 4) dragMoved = true
    alpha = Math.max(alpha, 0.18)
    scheduleFrame()
    return
  }
  if (dragging) {
    tx += mx - lastMx
    ty += my - lastMy
    lastMx = mx
    lastMy = my
    dragMoved = true
    scheduleFrame()
    return
  }

  // Hover
  const pIdx = hitPaper(mx, my)
  hoverPaperIdx = pIdx
  if (pIdx != null) {
    const d = data.value!
    hoverInfo.value = {
      kind: 'paper',
      title: d.papers[pIdx].title,
      sub: `${t('embedMap.degreeCount', { n: graphInfo.value?.degrees[pIdx] ?? 0 })} · ${t('embedMap.chunkCount', { n: d.papers[pIdx].chunk_count })}`,
      x: mx,
      y: my,
    }
  } else {
    const cIdx = hitChunk(mx, my)
    if (cIdx != null) {
      const d = data.value!
      const ch = d.chunks[cIdx]
      const typeLabel = sourceTypeLabel(ch.source_type)
      hoverInfo.value = {
        kind: 'chunk',
        title: ch.source_label || `${d.papers[ch.paper].title}`,
        sub: `[${typeLabel}] ${ch.preview}`,
        x: mx,
        y: my,
      }
    } else {
      hoverInfo.value = null
    }
  }
  canvasRef.value!.style.cursor = pIdx != null ? 'pointer' : 'default'
  scheduleFrame()
}

function onPointerUp(e: PointerEvent) {
  if (dragNode != null && !dragMoved) {
    selectedIdx.value = dragNode === selectedIdx.value ? null : dragNode
  } else if (dragging && !dragMoved) {
    selectedIdx.value = null
  }
  dragNode = null
  dragging = false
  try { canvasRef.value!.releasePointerCapture(e.pointerId) } catch { /* already released */ }
  scheduleFrame()
}

function onWheel(e: WheelEvent) {
  e.preventDefault()
  const rect = canvasRef.value!.getBoundingClientRect()
  const mx = e.clientX - rect.left
  const my = e.clientY - rect.top
  const factor = Math.exp(-e.deltaY * 0.0016)
  const next = Math.min(8, Math.max(0.12, scale * factor))
  const applied = next / scale
  tx = mx - applied * (mx - tx)
  ty = my - applied * (my - ty)
  scale = next
  scheduleFrame()
}

function onDblClick(e: MouseEvent) {
  const rect = canvasRef.value!.getBoundingClientRect()
  const hit = hitPaper(e.clientX - rect.left, e.clientY - rect.top)
  if (hit != null && data.value) openInMain(data.value.papers[hit])
}

function sourceTypeLabel(st: string): string {
  switch (st) {
    case 'metadata': return t('embedMap.srcMetadata')
    case 'highlight': return t('embedMap.srcHighlight')
    case 'note': return t('embedMap.srcNote')
    default: return t('embedMap.srcText')
  }
}

// ── Actions ───────────────────────────────────────────────────────────────────
async function load() {
  loading.value = true
  error.value = ''
  try {
    data.value = await invoke<EmbeddingMapData>('get_embedding_map', {
      model: selectedModel.value || null,
    })
    // Sync the picker to whatever model the backend actually rendered.
    selectedModel.value = data.value.embedding_model ?? ''
    selectedIdx.value = null
    initLayout()
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

// Re-fetch the map for the chosen embedding model so its vectors can be compared.
function onModelChange() {
  load()
}

function resetView() {
  fitView()
  scheduleFrame()
}

function selectNeighbor(idx: number) {
  selectedIdx.value = idx
  // center on it
  const [sx, sy] = toScreen(px[idx], py[idx])
  tx += cssW / 2 - sx
  ty += cssH / 2 - sy
  scheduleFrame()
}

async function openInMain(p: EmbeddingMapPaper) {
  await emitTo('main', 'argus-open-paper', { slug: p.slug, title: p.title }).catch(() => {})
  await invoke('focus_main_window').catch(() => {})
  window.setTimeout(() => { invoke('focus_main_window').catch(() => {}) }, 120)
}

async function openLibraryChat() {
  try { await invoke('open_library_chat_window') } catch { /* ignore */ }
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────
let resizeObserver: ResizeObserver | null = null

function syncCanvasSize() {
  const wrap = wrapRef.value
  const canvas = canvasRef.value
  if (!wrap || !canvas) return
  const dpr = window.devicePixelRatio || 1
  cssW = wrap.clientWidth
  cssH = wrap.clientHeight
  canvas.width = Math.round(cssW * dpr)
  canvas.height = Math.round(cssH * dpr)
  canvas.style.width = `${cssW}px`
  canvas.style.height = `${cssH}px`
  ctx = canvas.getContext('2d')
  if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  scheduleFrame()
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    // Clear the search first, then the selection on a second press.
    if (searchQuery.value) searchQuery.value = ''
    else selectedIdx.value = null
    scheduleFrame()
  }
}

watch([simThreshold, showChunks], ([thr, sc]) => {
  try {
    localStorage.setItem(THRESHOLD_KEY, String(thr))
    localStorage.setItem(SHOW_CHUNKS_KEY, String(sc))
  } catch { /* storage unavailable */ }
  scheduleFrame()
})
watch(selectedIdx, () => scheduleFrame())

// Redraw on every search change; once typing settles, frame the matches.
let searchFitTimer: ReturnType<typeof setTimeout> | null = null
watch(searchMatches, (m) => {
  scheduleFrame()
  if (searchFitTimer) clearTimeout(searchFitTimer)
  if (!m || m.size === 0) return
  searchFitTimer = setTimeout(() => fitNodes([...m]), 300)
})

onMounted(async () => {
  readTheme()
  syncCanvasSize()
  resizeObserver = new ResizeObserver(() => syncCanvasSize())
  if (wrapRef.value) resizeObserver.observe(wrapRef.value)
  window.addEventListener('keydown', onKeydown)
  await load()
})

onUnmounted(() => {
  if (rafId) cancelAnimationFrame(rafId)
  if (searchFitTimer) clearTimeout(searchFitTimer)
  resizeObserver?.disconnect()
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div class="embed-map-view">
    <!-- ── Titlebar (drag region) ── -->
    <div class="em-titlebar" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <div class="em-avatar" data-tauri-drag-region>
        <Icon icon="fluent:data-scatter-24-regular" width="15" height="15" data-tauri-drag-region />
      </div>
      <div class="em-title-block" data-tauri-drag-region>
        <span class="em-title" data-tauri-drag-region>{{ t('embedMap.title') }}</span>
        <span class="em-subtitle" data-tauri-drag-region>{{ stats }}</span>
      </div>
      <div class="em-titlebar-fill" data-tauri-drag-region />
    </div>

    <!-- ── Canvas area ── -->
    <div ref="wrapRef" class="em-canvas-wrap">
      <canvas
        ref="canvasRef"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
        @wheel="onWheel"
        @dblclick="onDblClick"
      />

      <!-- Controls -->
      <div v-if="!loading && data && data.papers.length" class="em-controls">
        <div class="em-control-row">
          <span class="em-control-label">{{ t('embedMap.search') }}</span>
          <div class="em-search-wrap">
            <input
              v-model="searchQuery"
              class="em-search-input"
              type="text"
              :placeholder="t('embedMap.searchPlaceholder')"
              @keydown.escape.stop="searchQuery = ''"
            />
            <button v-if="searchQuery" class="em-search-clear" @click="searchQuery = ''">
              <Icon icon="fluent:dismiss-24-regular" width="11" height="11" />
            </button>
          </div>
        </div>
        <div v-if="searchQuery.trim()" class="em-search-count">
          {{ t('embedMap.searchCount', { n: searchMatches?.size ?? 0 }) }}
        </div>
        <div v-if="data.available_models.length > 0" class="em-control-row">
          <span class="em-control-label">{{ t('embedMap.model') }}</span>
          <select v-model="selectedModel" class="em-model-select" @change="onModelChange">
            <option v-for="m in data.available_models" :key="m.embedding_model" :value="m.embedding_model">
              {{ m.embedding_model }} · {{ m.dimension }}d · {{ m.unique_papers }}
            </option>
          </select>
        </div>
        <div class="em-control-row">
          <span class="em-control-label">{{ t('embedMap.threshold') }}</span>
          <input v-model.number="simThreshold" type="range" min="0.25" max="0.95" step="0.01" class="em-slider" />
          <input
            class="em-threshold-input"
            type="text"
            inputmode="decimal"
            :value="simThreshold.toFixed(2)"
            @change="onThresholdInput"
            @keydown.enter="blurTarget"
          />
        </div>
        <label class="em-control-row em-checkbox-row">
          <input v-model="showChunks" type="checkbox" />
          <span class="em-control-label">{{ t('embedMap.showChunks') }}</span>
        </label>
        <div class="em-btn-row">
          <button class="em-mini-btn" @click="resetView">
            <Icon icon="fluent:arrow-maximize-24-regular" width="12" height="12" />
            {{ t('embedMap.resetView') }}
          </button>
          <button class="em-mini-btn" :disabled="loading" @click="load">
            <Icon icon="fluent:arrow-sync-24-regular" width="13" height="13" />
            {{ t('embedMap.refresh') }}
          </button>
        </div>
      </div>

      <!-- Hover tooltip -->
      <div
        v-if="hoverInfo"
        class="em-tooltip"
        :style="{ left: `${Math.max(8, Math.min(hoverInfo.x + 14, cssW - 280))}px`, top: `${hoverInfo.y + 14 + 80 > cssH ? Math.max(8, hoverInfo.y - 80) : hoverInfo.y + 14}px` }"
      >
        <div class="em-tooltip-title">{{ hoverInfo.title }}</div>
        <div class="em-tooltip-sub">{{ hoverInfo.sub }}</div>
      </div>

      <!-- Selected paper panel -->
      <div v-if="selectedPaper" class="em-panel">
        <div class="em-panel-header">
          <span class="em-panel-dot" :style="{ background: paperColor(selectedIdx!, 1) }" />
          <span class="em-panel-title">{{ selectedPaper.title }}</span>
          <button class="em-icon-btn em-panel-close" @click="selectedIdx = null">
            <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
          </button>
        </div>
        <div class="em-panel-meta">
          {{ t('embedMap.degreeCount', { n: graphInfo?.degrees[selectedIdx!] ?? 0 }) }} · {{ t('embedMap.chunkCount', { n: selectedPaper.chunk_count }) }}
        </div>
        <button class="em-open-btn" @click="openInMain(selectedPaper)">
          <Icon icon="fluent:open-24-regular" width="12" height="12" />
          {{ t('embedMap.openInMain') }}
        </button>
        <div v-if="selectedNeighbors.length" class="em-neighbors">
          <div class="em-neighbors-title">{{ t('embedMap.neighbors') }}</div>
          <button
            v-for="nb in selectedNeighbors"
            :key="nb.idx"
            class="em-neighbor-item"
            @click="selectNeighbor(nb.idx)"
          >
            <span class="em-neighbor-dot" :style="{ background: paperColor(nb.idx, 1) }" />
            <span class="em-neighbor-name">{{ nb.title }}</span>
            <span class="em-neighbor-sim">{{ (nb.sim * 100).toFixed(0) }}%</span>
          </button>
        </div>
      </div>

      <!-- Loading -->
      <div v-if="loading" class="em-center-state">
        <span class="em-spinner" />
        <p>{{ t('embedMap.loading') }}</p>
      </div>

      <!-- Error -->
      <div v-else-if="error" class="em-center-state">
        <p class="em-error-text">{{ error }}</p>
        <button class="em-mini-btn" @click="load">{{ t('embedMap.refresh') }}</button>
      </div>

      <!-- Empty -->
      <div v-else-if="data && !data.papers.length" class="em-center-state">
        <Icon icon="fluent:data-scatter-24-regular" class="em-empty-icon" width="44" height="44" />
        <p class="em-empty-title">{{ t('embedMap.empty') }}</p>
        <p class="em-empty-hint">{{ t('embedMap.emptyHint') }}</p>
        <div class="em-empty-actions">
          <button class="em-accent-btn" @click="openLibraryChat">{{ t('embedMap.openChat') }}</button>
          <button class="em-mini-btn" @click="load">{{ t('embedMap.refresh') }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.embed-map-view {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--bg-primary);
  color: var(--text-primary);
  font-family: var(--font-sans);
}

/* ── Titlebar ── */
.em-titlebar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 44px;
  flex-shrink: 0;
  padding: 0 14px 0 0;
  border-bottom: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--bg-primary) 85%, var(--bg-secondary));
}
.em-titlebar .tl-space { width: 96px; flex-shrink: 0; }
.em-avatar {
  width: 26px;
  height: 26px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  background: var(--accent-light);
  flex-shrink: 0;
}
.em-title-block {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}
.em-title {
  font-size: 13px;
  font-weight: 700;
  line-height: 1.2;
}
.em-subtitle {
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.em-titlebar-fill { flex: 1 1 auto; height: 100%; }

.em-icon-btn {
  width: 26px;
  height: 26px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
  flex-shrink: 0;
  transition: background 0.12s, color 0.12s;
}
.em-icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.em-icon-btn:disabled { opacity: 0.5; cursor: default; }
@keyframes em-rotate { to { transform: rotate(360deg); } }

/* ── Canvas ── */
.em-canvas-wrap {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.em-canvas-wrap canvas {
  display: block;
  touch-action: none;
}

/* ── Floating controls ── */
.em-controls {
  position: absolute;
  top: 12px;
  left: 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-primary) 88%, transparent);
  backdrop-filter: blur(8px);
  border: 1px solid var(--border-subtle);
  box-shadow: var(--shadow-md);
}
.em-control-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
}
.em-control-label { color: var(--text-secondary); white-space: nowrap; }
.em-threshold-input {
  width: 46px;
  padding: 2px 5px;
  font-size: 11px;
  text-align: center;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-variant-numeric: tabular-nums;
  transition: border-color 0.12s;
}
.em-threshold-input:focus {
  outline: none;
  border-color: var(--accent);
  background: var(--bg-primary);
}
.em-slider { width: 110px; accent-color: var(--accent); }
.em-model-select {
  flex: 1;
  min-width: 0;
  max-width: 220px;
  padding: 3px 6px;
  font-size: 11px;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.em-model-select:focus { outline: none; border-color: var(--accent); }
.em-search-wrap {
  position: relative;
  flex: 1;
  min-width: 0;
  max-width: 220px;
  display: flex;
  align-items: center;
}
.em-search-input {
  width: 100%;
  padding: 3px 22px 3px 7px;
  font-size: 11px;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
}
.em-search-input:focus { outline: none; border-color: var(--accent); background: var(--bg-primary); }
.em-search-clear {
  position: absolute;
  right: 5px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-tertiary);
  flex-shrink: 0;
}
.em-search-clear:hover { color: var(--text-primary); background: var(--bg-hover); }
.em-search-count {
  font-size: 11px;
  color: var(--text-tertiary);
  padding-left: 2px;
}
.em-checkbox-row { cursor: pointer; user-select: none; }
.em-checkbox-row input { accent-color: var(--accent); }

.em-btn-row {
  display: flex;
  gap: 6px;
}

.em-mini-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 10px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  transition: background 0.12s;
}
.em-mini-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
.em-mini-btn:disabled { opacity: 0.5; cursor: default; }
.em-mini-btn svg { flex-shrink: 0; opacity: 0.8; }

.em-empty-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 4px;
}

/* ── Tooltip ── */
.em-tooltip {
  position: absolute;
  max-width: 280px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-primary) 94%, transparent);
  border: 1px solid var(--border-default);
  box-shadow: var(--shadow-md);
  pointer-events: none;
  z-index: 20;
}
.em-tooltip-title {
  font-size: 12px;
  font-weight: 600;
  line-height: 1.35;
  word-break: break-word;
}
.em-tooltip-sub {
  margin-top: 3px;
  font-size: 11px;
  color: var(--text-tertiary);
  line-height: 1.4;
  word-break: break-word;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* ── Selected paper panel ── */
.em-panel {
  position: absolute;
  top: 12px;
  right: 14px;
  width: 264px;
  max-height: calc(100% - 24px);
  overflow-y: auto;
  padding: 12px 14px;
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-primary) 92%, transparent);
  backdrop-filter: blur(8px);
  border: 1px solid var(--border-subtle);
  box-shadow: var(--shadow-md);
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.em-panel-header {
  display: flex;
  align-items: flex-start;
  gap: 7px;
}
.em-panel-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  flex-shrink: 0;
  margin-top: 4px;
}
.em-panel-title {
  flex: 1;
  font-size: 12.5px;
  font-weight: 650;
  line-height: 1.4;
  word-break: break-word;
}
.em-panel-close { width: 20px; height: 20px; }
.em-panel-meta {
  font-size: 11px;
  color: var(--text-tertiary);
}
.em-open-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 5px 10px;
  font-size: 12px;
  font-weight: 600;
  color: var(--accent);
  border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent);
  border-radius: var(--radius-md);
  transition: background 0.12s;
}
.em-open-btn:hover { background: var(--accent-light); }

.em-neighbors {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.em-neighbors-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  margin: 4px 0 3px;
}
.em-neighbor-item {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 5px 6px;
  border-radius: var(--radius-sm);
  font-size: 12px;
  text-align: left;
  transition: background 0.1s;
  min-width: 0;
}
.em-neighbor-item:hover { background: var(--bg-hover); }
.em-neighbor-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}
.em-neighbor-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
}
.em-neighbor-sim {
  font-size: 11px;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

/* ── Center states ── */
.em-center-state {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-secondary);
  font-size: 13px;
  pointer-events: none;
}
.em-center-state button { pointer-events: auto; }
.em-empty-icon { color: var(--text-tertiary); opacity: 0.7; }
.em-empty-title { font-size: 14px; font-weight: 600; color: var(--text-primary); }
.em-empty-hint { font-size: 12px; color: var(--text-tertiary); max-width: 320px; text-align: center; line-height: 1.5; }
.em-error-text {
  max-width: 420px;
  text-align: center;
  font-size: 12px;
  color: #cc3333;
  line-height: 1.5;
  word-break: break-word;
}
.em-accent-btn {
  margin-top: 4px;
  padding: 6px 16px;
  font-size: 12.5px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-pill);
  transition: opacity 0.12s;
}
.em-accent-btn:hover { opacity: 0.85; }

.em-spinner {
  width: 18px;
  height: 18px;
  border: 2px solid var(--border-default);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: em-rotate 0.8s linear infinite;
}
</style>
