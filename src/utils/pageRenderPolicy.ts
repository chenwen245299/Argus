// Which PDF pages should be rendered, which should be thrown away, and in what order.
//
// Pure: no Vue, no DOM, no timers. PdfViewer.vue feeds it the page geometry and what it
// currently holds, and carries out the plan. Keeping the decisions here is what makes them
// testable without a browser (see the node:test suite that ships with the change).
//
// The shape of the policy, and why:
//
//  - RENDER ZONE: the pages worth having ready. It is measured in viewports, not pixels,
//    because the cost of arriving at a blank page is "how many frames until I see it", and
//    that scales with how far the user moves per frame, i.e. with the viewport. It leans in
//    the direction of travel (1.5 - 2.5 viewports ahead, faster scrolling reaches further)
//    and keeps one viewport behind. A fixed 600 px margin - less than ONE page at 189 % zoom
//    - could only ever start a page when it was already about to be seen.
//  - KEEP ZONE: the render zone plus a wide extra ring. A page is thrown away only when it
//    leaves the keep zone, so a page right at the edge of the render zone cannot flap between
//    "evicted" and "rendering again" as the scroll position wobbles around that edge.
//  - PRIORITY: visible pages first, then the side the user is heading to, then the other side.
//    Every page in the zone used to start at once in whatever order the observer reported
//    them, so the page actually on screen could queue behind a prefetch.
//  - CANCELLATION: work that has not started is dropped as soon as its page leaves the render
//    zone (nobody is waiting for it); work already running is only cancelled once its page
//    leaves the keep zone (it is part-way done and the user may well scroll back).
//  - BUDGET: bitmaps are the memory that matters. If the keep zone holds more pixels than the
//    budget the farthest pages go first - never a visible page, never one inside the render
//    zone, never one somebody explicitly asked for (`pinned`), never one whose render is running.

export interface PageGeometry {
  /** Top edge of every page in scroll-content coordinates (CSS px), ascending. */
  tops: ArrayLike<number>
  /** Height of every page (CSS px). */
  heights: ArrayLike<number>
}

export interface PolicyTuning {
  /** Viewports of look-ahead in the travel direction when scrolling slowly / fast. */
  aheadMinVh: number
  aheadMaxVh: number
  /** Scroll speed (viewports per second) at which the look-ahead reaches `aheadMaxVh`. */
  fastVhPerSec: number
  /** Viewports kept rendered behind the direction of travel. */
  behindVh: number
  /** Look-ahead below / above the viewport when there is no direction (idle, just opened, jumped). */
  idleBelowVh: number
  idleAboveVh: number
  /** Extra viewports around the render zone inside which a page is NOT thrown away. */
  keepMarginVh: number
  /** A viewport shorter than this (px) is treated as this tall (a collapsing pane must not shrink the zone to nothing). */
  minViewportPx: number
}

export const DEFAULT_TUNING: PolicyTuning = {
  aheadMinVh: 1.5,
  aheadMaxVh: 2.5,
  fastVhPerSec: 4,
  behindVh: 1,
  idleBelowVh: 1.5,
  idleAboveVh: 1,
  keepMarginVh: 2,
  minViewportPx: 300,
}

export interface RenderPolicyInput {
  geometry: PageGeometry
  scrollTop: number
  viewportHeight: number
  /** Sign of the recent scroll movement: 1 down, -1 up, 0 none / just jumped. */
  direction: -1 | 0 | 1
  /** Absolute scroll speed in px per ms (0 when idle). */
  speed: number
  /** Pages whose DOM is complete, at whatever scale. */
  rendered: ReadonlySet<number>
  /** The subset of `rendered` that was built for another scale / devicePixelRatio. */
  stale?: ReadonlySet<number>
  /** Renders that have started and not finished. */
  inFlight?: ReadonlySet<number>
  /** Renders that are waiting for a slot. */
  queued?: ReadonlySet<number>
  /** Pages somebody explicitly asked for (jump target, search hit): rendered, never cancelled or evicted. */
  pinned?: ReadonlySet<number>
  /** Bitmap pixels a page costs (what it holds when rendered, or will hold once it is). */
  pagePixels: (page: number) => number
  /** Most bitmap pixels the viewer may hold. Never forces out visible / render-zone / pinned pages. */
  budgetPixels: number
  tuning?: Partial<PolicyTuning>
}

export interface PageRange {
  first: number
  last: number
}

export interface RenderPlan {
  /** Pages intersecting the viewport, ascending. */
  visible: number[]
  /** Pages to start, best first. Excludes pages that are fresh or already running. */
  render: number[]
  /** Queued pages that left the render zone: forget them. */
  dropQueued: number[]
  /** Running renders whose page left the keep zone: cancel them and discard what they built. */
  cancelInFlight: number[]
  /** Rendered pages to throw away. */
  evict: number[]
  zone: PageRange | null
  keep: PageRange | null
  /** Bitmap pixels held (rendered + in flight) once this plan is carried out. */
  retainedPixels: number
}

const EMPTY_SET: ReadonlySet<number> = new Set()

function clamp01(x: number): number {
  return x < 0 ? 0 : x > 1 ? 1 : x
}

/** Smallest page index whose bottom edge is below `y`, or `n` when there is none. */
function firstEndingAfter(g: PageGeometry, y: number): number {
  let lo = 0
  let hi = g.tops.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (g.tops[mid] + g.heights[mid] > y) hi = mid
    else lo = mid + 1
  }
  return lo
}

/** Largest page index whose top edge is above `y`, or -1 when there is none. */
function lastStartingBefore(g: PageGeometry, y: number): number {
  let lo = 0
  let hi = g.tops.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if (g.tops[mid] < y) lo = mid + 1
    else hi = mid
  }
  return lo - 1
}

/** Pages touching the vertical span [y0, y1) as an inclusive index range, or null. */
export function pageRangeIn(g: PageGeometry, y0: number, y1: number): PageRange | null {
  const n = g.tops.length
  if (n === 0 || y1 <= y0) return null
  const first = firstEndingAfter(g, y0)
  const last = lastStartingBefore(g, y1)
  if (first >= n || last < 0 || first > last) return null
  return { first, last }
}

/** Gap in px between the span [y0, y1] and page `i` (0 when they overlap). */
function distanceTo(g: PageGeometry, i: number, y0: number, y1: number): number {
  const top = g.tops[i]
  const bottom = top + g.heights[i]
  if (bottom <= y0) return y0 - bottom
  if (top >= y1) return top - y1
  return 0
}

function overlapWith(g: PageGeometry, i: number, y0: number, y1: number): number {
  const top = g.tops[i]
  return Math.max(0, Math.min(top + g.heights[i], y1) - Math.max(top, y0))
}

const EMPTY_PLAN: RenderPlan = {
  visible: [], render: [], dropQueued: [], cancelInFlight: [], evict: [], zone: null, keep: null, retainedPixels: 0,
}

/** The render zone and the keep zone around a scroll position, in page-index ranges. */
export function zonesFor(
  g: PageGeometry,
  scrollTop: number,
  viewportHeight: number,
  direction: -1 | 0 | 1,
  speed: number,
  tuning: Partial<PolicyTuning> = {},
): { visible: PageRange | null; zone: PageRange | null; keep: PageRange | null } {
  const t = { ...DEFAULT_TUNING, ...tuning }
  const vh = Math.max(viewportHeight, t.minViewportPx)
  const vTop = scrollTop
  const vBottom = scrollTop + viewportHeight

  let aheadPx: number
  let behindPx: number
  if (direction === 0) {
    aheadPx = t.idleBelowVh * vh
    behindPx = t.idleAboveVh * vh
  } else {
    const speedVh = (speed * 1000) / vh
    aheadPx = (t.aheadMinVh + (t.aheadMaxVh - t.aheadMinVh) * clamp01(speedVh / t.fastVhPerSec)) * vh
    behindPx = t.behindVh * vh
  }
  const down = direction >= 0
  const zoneTop = vTop - (down ? behindPx : aheadPx)
  const zoneBottom = vBottom + (down ? aheadPx : behindPx)
  const keepPx = t.keepMarginVh * vh
  return {
    visible: pageRangeIn(g, vTop, vBottom),
    zone: pageRangeIn(g, zoneTop, zoneBottom),
    keep: pageRangeIn(g, zoneTop - keepPx, zoneBottom + keepPx),
  }
}

/**
 * Decide what the viewer should do right now. Calling it again with unchanged inputs gives
 * the same answer, and carrying the plan out and calling it again gives an empty plan
 * (apart from the pages still waiting for their turn) - there is no state in here.
 */
export function planPageRender(input: RenderPolicyInput): RenderPlan {
  const g = input.geometry
  const n = g.tops.length
  if (n === 0 || input.viewportHeight <= 0) return EMPTY_PLAN
  const stale = input.stale ?? EMPTY_SET
  const inFlight = input.inFlight ?? EMPTY_SET
  const queued = input.queued ?? EMPTY_SET
  const pinned = input.pinned ?? EMPTY_SET
  const vTop = input.scrollTop
  const vBottom = input.scrollTop + input.viewportHeight

  const { visible: vis, zone, keep } = zonesFor(
    g, input.scrollTop, input.viewportHeight, input.direction, input.speed, input.tuning,
  )
  const inRange = (r: PageRange | null, i: number) => !!r && i >= r.first && i <= r.last
  const visible: number[] = []
  if (vis) for (let i = vis.first; i <= vis.last; i++) visible.push(i)

  // ── what to start, best first ────────────────────────────────────────────────────
  const down = input.direction >= 0
  const wanted: { page: number; rank: number; dist: number }[] = []
  const needs = (i: number) => (!input.rendered.has(i) || stale.has(i)) && !inFlight.has(i)
  for (const i of pinned) {
    if (i >= 0 && i < n && needs(i)) wanted.push({ page: i, rank: 0, dist: 0 })
  }
  if (zone) {
    for (let i = zone.first; i <= zone.last; i++) {
      if (pinned.has(i) || !needs(i)) continue
      if (inRange(vis, i)) {
        // The page covering most of the screen first; ties go to the side of travel.
        const share = overlapWith(g, i, vTop, vBottom)
        wanted.push({ page: i, rank: 1, dist: -share })
      } else {
        const below = g.tops[i] >= vBottom
        const ahead = down ? below : !below
        wanted.push({ page: i, rank: ahead ? 2 : 3, dist: distanceTo(g, i, vTop, vBottom) })
      }
    }
  }
  wanted.sort((a, b) => a.rank - b.rank || a.dist - b.dist || (down ? a.page - b.page : b.page - a.page))

  // ── what to forget ───────────────────────────────────────────────────────────────
  const dropQueued: number[] = []
  for (const i of queued) if (!inRange(zone, i) && !pinned.has(i)) dropQueued.push(i)
  const cancelInFlight: number[] = []
  for (const i of inFlight) if (!inRange(keep, i) && !pinned.has(i)) cancelInFlight.push(i)

  // A page whose render is still running is never evicted: eviction empties its wrapper while the
  // render goes on to append its remaining layers to it and then marks the page fresh, which
  // leaves a blank page the plan believes is done. A running render that has left the keep zone
  // is in `cancelInFlight` instead; the page is evicted by the plan after it has unwound.
  const evictSet = new Set<number>()
  for (const i of input.rendered) {
    if (pinned.has(i) || inRange(vis, i) || inFlight.has(i)) continue
    if (!inRange(keep, i)) evictSet.add(i)
    // A stale page outside the render zone is only a stretched bitmap nobody is looking at.
    else if (stale.has(i) && !inRange(zone, i)) evictSet.add(i)
  }

  // ── memory budget ────────────────────────────────────────────────────────────────
  let retained = 0
  const holding: number[] = []
  for (const i of input.rendered) if (!evictSet.has(i)) { retained += input.pagePixels(i); holding.push(i) }
  for (const i of inFlight) if (!input.rendered.has(i) && !cancelInFlight.includes(i)) retained += input.pagePixels(i)
  if (retained > input.budgetPixels) {
    const candidates = holding
      .filter(i => !pinned.has(i) && !inRange(zone, i) && !inRange(vis, i) && !inFlight.has(i))
      .map(i => ({ page: i, dist: distanceTo(g, i, vTop, vBottom) }))
      .sort((a, b) => b.dist - a.dist || b.page - a.page)
    for (const c of candidates) {
      if (retained <= input.budgetPixels) break
      evictSet.add(c.page)
      retained -= input.pagePixels(c.page)
    }
  }

  return {
    visible,
    render: wanted.map(w => w.page),
    dropQueued,
    cancelInFlight,
    evict: [...evictSet].sort((a, b) => a - b),
    zone,
    keep,
    retainedPixels: retained,
  }
}

// ── Scroll speed and direction ────────────────────────────────────────────────────────

/**
 * Turns scroll positions into a direction and a speed. Smoothed over a few events (a single
 * wheel tick is not a trend), sticky in direction once the movement stops (reading carries on
 * the way it was going), and a jump of several viewports at once (jump to page, dragging the
 * thumb, a link) resets the direction: where the user lands says nothing about where they head.
 */
export class ScrollTracker {
  /** Sign of the last significant movement; 0 until there is one or after a jump. */
  direction: -1 | 0 | 1 = 0
  private smoothed = 0
  private lastTop: number | null = null
  private lastT = 0
  private lastMoveT = -Infinity

  private readonly jumpViewports: number
  private readonly idleMs: number

  constructor(jumpViewports = 3, idleMs = 150) {
    this.jumpViewports = jumpViewports
    this.idleMs = idleMs
  }

  update(scrollTop: number, now: number, viewportHeight: number): void {
    if (this.lastTop === null) {
      this.lastTop = scrollTop
      this.lastT = now
      return
    }
    const dy = scrollTop - this.lastTop
    const dt = Math.max(1, now - this.lastT)
    this.lastTop = scrollTop
    this.lastT = now
    if (dy === 0) return
    if (Math.abs(dy) > this.jumpViewports * Math.max(viewportHeight, 1)) {
      this.direction = 0
      this.smoothed = 0
      this.lastMoveT = now
      return
    }
    const inst = Math.abs(dy) / dt
    this.smoothed = dt < 100 ? this.smoothed * 0.6 + inst * 0.4 : inst
    this.direction = dy > 0 ? 1 : -1
    this.lastMoveT = now
  }

  /** Smoothed speed in px per ms; 0 once the scroll has been still for a moment. */
  speed(now: number): number {
    return now - this.lastMoveT > this.idleMs ? 0 : this.smoothed
  }

  /** Forget the position (the layout changed under it: zoom, resize). Direction is kept. */
  rebase(): void {
    this.lastTop = null
    this.smoothed = 0
  }
}

// ── Viewers that are not on screen ─────────────────────────────────────────────────────
//
// A background tab keeps every canvas it had, which is the point (switching back must not
// re-render), but a pile of tabs must not hold a pile of 30 MB bitmaps. Trimming is by LRU
// across the tabs: right after hiding, a tab drops what lies outside the zone it will show
// first (level 1); under pressure the longest-hidden tabs shrink to the pages that were on
// screen (level 2) and then to nothing (level 3).

export type HiddenLevel = 0 | 1 | 2 | 3

/** Pages a hidden viewer keeps at each level, given the scroll position it will return to. */
export function hiddenKeepSet(
  g: PageGeometry,
  scrollTop: number,
  viewportHeight: number,
  level: HiddenLevel,
  tuning: Partial<PolicyTuning> = {},
): Set<number> {
  const out = new Set<number>()
  if (level === 0) return out // callers treat 0 as "keep everything"; nothing to compute
  if (level === 3 || g.tops.length === 0 || viewportHeight <= 0) return out
  const z = zonesFor(g, scrollTop, viewportHeight, 0, 0, tuning)
  const r = level === 1 ? z.zone : z.visible
  if (r) for (let i = r.first; i <= r.last; i++) out.add(i)
  return out
}

export interface HiddenViewerState {
  id: string
  /** performance.now() when it was hidden; smaller = hidden longer = trimmed first. */
  hiddenSince: number
  /** Level already applied. */
  level: HiddenLevel
  /** Bitmap pixels it would hold at level 1, 2 and 3 (index 0 = level 1). */
  pixelsAtLevel: [number, number, number]
  /** Pixels it holds right now (used while level is still 0). */
  pixelsNow: number
}

/** The level each hidden viewer should be trimmed to so the total fits the budget. */
export function planHiddenBudget(viewers: HiddenViewerState[], budgetPixels: number): Map<string, HiddenLevel> {
  const target = new Map<string, HiddenLevel>()
  const pixelsOf = (v: HiddenViewerState, level: HiddenLevel) => (level === 0 ? v.pixelsNow : v.pixelsAtLevel[level - 1])
  // Every hidden viewer is at least at level 1: nothing outside its return zone is worth holding.
  for (const v of viewers) target.set(v.id, Math.max(1, v.level) as HiddenLevel)
  const total = () => viewers.reduce((s, v) => s + pixelsOf(v, target.get(v.id)!), 0)
  const oldestFirst = [...viewers].sort((a, b) => a.hiddenSince - b.hiddenSince)
  while (total() > budgetPixels) {
    const v = oldestFirst.find(x => target.get(x.id)! < 3)
    if (!v) break
    target.set(v.id, (target.get(v.id)! + 1) as HiddenLevel)
  }
  return target
}
