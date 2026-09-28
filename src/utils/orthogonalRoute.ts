/**
 * Orthogonal routing for canvas edges.
 *
 * Canvas edges are always polylines — horizontal and vertical runs joined by the
 * same small rounded corner as vue-flow's smooth-step edge — never free curves.
 * An edge without control points is drawn by `getSmoothStepPath`; one with
 * control points is routed here, through the points the user placed.
 *
 * Routing (`routeOrthogonal`) goes in three steps:
 * 1. Each control point is settled (`placeControlPoint`): put on a neighbour's
 *    row or column when it is within `JOG` of it, and pushed out of the short
 *    straight run every line starts and ends with at a handle.
 * 2. Between two consecutive anchors — the end of the source's straight run, the
 *    control points, the start of the target's — the line is an elbow:
 *    horizontal-then-vertical or vertical-then-horizontal. Every combination is
 *    scored (`scoreLine`) and the cheapest is drawn: fewest bends, a run doubling
 *    back on itself only when nothing else passes through the points, and as
 *    tie-breaks no bend right where a handle's straight run ends (the line would
 *    hug the node) and no narrow U-turn.
 * 3. Steps too small to be deliberate are ironed out (`absorbJogs`).
 *
 * The rest keeps an edge's shape while the user edits it: `planImplicitDrag` and
 * `insertControlPoint` pick control points that reproduce the line already on
 * screen, so grabbing or double-clicking an edge never makes it jump.
 *
 * Kept free of runtime imports so it can be exercised outside the app.
 */

export type HandleSide = 'left' | 'right' | 'top' | 'bottom'

export interface RoutePoint {
  x: number
  y: number
}

/** Where an edge starts and ends, and which side of its node each handle sits on. */
export interface EdgeEnds {
  source: RoutePoint
  sourceSide?: HandleSide
  target: RoutePoint
  targetSide?: HandleSide
}

export interface OrthogonalRoute {
  /** Vertices of the drawn polyline, source first and target last. */
  points: RoutePoint[]
  /**
   * The line cut at the control points, before steps were ironed out: `legs[i]`
   * runs from control point `i - 1` (the source when `i` is 0) to control point
   * `i` (the target for the last leg). A point dropped onto leg `i` belongs at
   * index `i` of the control-point list.
   */
  legs: RoutePoint[][]
  /** The corner each leg turns at, or null for a straight leg. */
  corners: Array<RoutePoint | null>
  /** The control points as routed, each settled by `placeControlPoint`. */
  waypoints: RoutePoint[]
}

/** Corner radius shared with the smooth-step default, so both kinds of edge look alike. */
export const EDGE_CORNER_RADIUS = 5
/** Straight run out of a handle before the first turn (vue-flow's smooth-step offset). */
export const EDGE_HANDLE_OFFSET = 20
/**
 * A control point this close to a neighbour's row or column counts as on it, and
 * a step this short between two runs heading the same way is ironed out: an
 * offset that small reads as a rendering glitch, not a choice.
 */
export const JOG = 8

// Headings, clockwise from +x.
type Dir = 0 | 1 | 2 | 3
const RIGHT: Dir = 0
const DOWN: Dir = 1
const LEFT: Dir = 2
const UP: Dir = 3
const DIRS: Dir[] = [RIGHT, DOWN, LEFT, UP]
const UNIT: RoutePoint[] = [{ x: 1, y: 0 }, { x: 0, y: 1 }, { x: -1, y: 0 }, { x: 0, y: -1 }]
const SIDE: Record<HandleSide, Dir> = { right: RIGHT, bottom: DOWN, left: LEFT, top: UP }

/** Below this an offset counts as none: the run is straight, not a sub-pixel jog. */
const EPS = 0.5
const BEND = 1
/** Doubling back along the same line reads as a spike; almost any number of bends is better. */
const REVERSAL = 100
/** Doubling back where a handle's straight run ends runs back into the node itself: worse still. */
const STUB_REVERSAL = 20
/** A bend right where a handle's straight run ends. Both ends together stay under one bend. */
const STUB_BEND = 0.3
/** A U-turn around a run narrower than HAIRPIN_RUN: two parallel lines drawn close together. */
const HAIRPIN = 0.5
const HAIRPIN_RUN = 40
/** Beyond this many two-way gaps, elbows are chosen gap by gap instead of scoring every combination. */
const MAX_EXHAUSTIVE_FORKS = 10

function sideDir(side: HandleSide | undefined, fallback: Dir): Dir {
  return side !== undefined && SIDE[side] !== undefined ? SIDE[side] : fallback
}

function isOpposite(a: Dir, b: Dir): boolean {
  return (a + 2) % 4 === b
}

function near(a: RoutePoint, b: RoutePoint): boolean {
  return Math.abs(a.x - b.x) < EPS && Math.abs(a.y - b.y) < EPS
}

function dist(a: RoutePoint, b: RoutePoint): number {
  return Math.hypot(b.x - a.x, b.y - a.y)
}

function headingOf(a: RoutePoint, b: RoutePoint): Dir | null {
  const dx = b.x - a.x
  const dy = b.y - a.y
  if (Math.abs(dy) < EPS && Math.abs(dx) >= EPS) return dx > 0 ? RIGHT : LEFT
  if (Math.abs(dx) < EPS && Math.abs(dy) >= EPS) return dy > 0 ? DOWN : UP
  return null
}

// ── Edge data ──────────────────────────────────────────────────────────────────

function isRoutePoint(point: unknown): point is RoutePoint {
  if (!point || typeof point !== 'object') return false
  const maybe = point as Partial<RoutePoint>
  return typeof maybe.x === 'number' && Number.isFinite(maybe.x) &&
    typeof maybe.y === 'number' && Number.isFinite(maybe.y)
}

/**
 * The control points an edge's data carries: its `controlPoints` list, or the
 * single `controlX`/`controlY` point older canvases saved.
 */
export function readControlPoints(
  data: { controlPoints?: unknown; controlX?: unknown; controlY?: unknown } | null | undefined,
): RoutePoint[] {
  if (Array.isArray(data?.controlPoints)) return data.controlPoints.filter(isRoutePoint)
  const legacy = { x: data?.controlX, y: data?.controlY }
  return isRoutePoint(legacy) ? [legacy] : []
}

export function edgeHasControlPoints(
  data: { controlPoints?: unknown; controlX?: unknown; controlY?: unknown } | null | undefined,
): boolean {
  return readControlPoints(data).length > 0
}

// ── Polylines ──────────────────────────────────────────────────────────────────

/** Drop repeated vertices and the joints of runs that carry straight on. */
function simplify(points: RoutePoint[]): RoutePoint[] {
  const out: RoutePoint[] = []
  for (const p of points) {
    const last = out[out.length - 1]
    if (last && near(last, p)) continue
    if (out.length >= 2) {
      const before = headingOf(out[out.length - 2], last)
      // Merge only while the merged run itself stays straight, so sub-pixel
      // drifts can't add up into a slant.
      if (before !== null && before === headingOf(last, p) && before === headingOf(out[out.length - 2], p)) {
        out[out.length - 1] = p
        continue
      }
    }
    out.push(p)
  }
  // Near-duplicates keep the earlier point, but the line must end exactly on the target.
  const end = points[points.length - 1]
  if (end && out.length > 1) out[out.length - 1] = end
  return out
}

interface LineHit {
  point: RoutePoint
  segment: number
  /** Distance along the line from its start. */
  arc: number
  distance: number
}

function closestOnSegment(p: RoutePoint, a: RoutePoint, b: RoutePoint): RoutePoint {
  const dx = b.x - a.x
  const dy = b.y - a.y
  const lengthSq = dx * dx + dy * dy
  const t = lengthSq === 0
    ? 0
    : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / lengthSq))
  return { x: a.x + t * dx, y: a.y + t * dy }
}

function project(line: RoutePoint[], p: RoutePoint): LineHit {
  let best: LineHit = { point: { ...(line[0] ?? p) }, segment: 0, arc: 0, distance: Infinity }
  let walked = 0
  for (let j = 0; j < line.length - 1; j += 1) {
    const point = closestOnSegment(p, line[j], line[j + 1])
    const distance = dist(p, point)
    if (distance < best.distance) best = { point, segment: j, arc: walked + dist(line[j], point), distance }
    walked += dist(line[j], line[j + 1])
  }
  return best
}

/** The nearest point of a polyline. */
export function closestPointOnLine(line: RoutePoint[], p: RoutePoint): RoutePoint {
  return project(line, p).point
}

function lineLength(line: RoutePoint[]): number {
  let total = 0
  for (let i = 0; i < line.length - 1; i += 1) total += dist(line[i], line[i + 1])
  return total
}

function pointAtArc(line: RoutePoint[], arc: number): RoutePoint {
  let remaining = arc
  for (let i = 0; i < line.length - 1; i += 1) {
    const len = dist(line[i], line[i + 1])
    if (len > 0 && remaining <= len) {
      const t = remaining / len
      return { x: line[i].x + (line[i + 1].x - line[i].x) * t, y: line[i].y + (line[i + 1].y - line[i].y) * t }
    }
    remaining -= len
  }
  return { ...line[line.length - 1] }
}

/** Whether two polylines draw the same line, vertex for vertex within `tolerance`. */
export function sameLine(a: RoutePoint[], b: RoutePoint[], tolerance = 1): boolean {
  const x = simplify(a)
  const y = simplify(b)
  return x.length === y.length &&
    x.every((p, i) => Math.abs(p.x - y[i].x) <= tolerance && Math.abs(p.y - y[i].y) <= tolerance)
}

/** SVG path for a polyline, each right-angle turn softened to `radius` like the smooth-step default. */
export function polylinePath(points: RoutePoint[], radius = EDGE_CORNER_RADIUS): string {
  if (points.length === 0) return ''
  let d = `M ${points[0].x},${points[0].y}`
  for (let i = 1; i < points.length - 1; i += 1) {
    const a = points[i - 1]
    const b = points[i]
    const c = points[i + 1]
    const inLen = dist(a, b)
    const outLen = dist(b, c)
    const r = Math.min(radius, inLen / 2, outLen / 2)
    if (!(r > 0.1)) {
      d += ` L ${b.x},${b.y}`
      continue
    }
    const ux = (b.x - a.x) / inLen
    const uy = (b.y - a.y) / inLen
    const wx = (c.x - b.x) / outLen
    const wy = (c.y - b.y) / outLen
    // Only a genuine turn is rounded; a run that doubles back stays sharp.
    if (Math.abs(ux * wx + uy * wy) > 0.5) {
      d += ` L ${b.x},${b.y}`
      continue
    }
    d += ` L ${b.x - ux * r},${b.y - uy * r} Q ${b.x},${b.y} ${b.x + wx * r},${b.y + wy * r}`
  }
  if (points.length > 1) {
    const last = points[points.length - 1]
    d += ` L ${last.x},${last.y}`
  }
  return d
}

/** The point halfway along a polyline, where its label sits. */
export function polylineMidpoint(points: RoutePoint[]): RoutePoint {
  if (points.length === 0) return { x: 0, y: 0 }
  return pointAtArc(points, lineLength(points) / 2)
}

/**
 * The vertices of a path from vue-flow's `getSmoothStepPath` — the line an edge
 * without control points is drawn as. Each rounded corner is an `L` leading into
 * a `Q` whose control point is the corner itself. Null for anything else, so a
 * change in vue-flow's output degrades to the old behaviour instead of guessing.
 */
export function smoothStepVertices(path: string): RoutePoint[] | null {
  const tokens = path.match(/[A-Za-z]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/g)
  if (!tokens) return null
  const commands: Array<{ op: string; values: number[] }> = []
  for (const token of tokens) {
    if (/^[A-Za-z]$/.test(token)) commands.push({ op: token, values: [] })
    else if (commands.length > 0) commands[commands.length - 1].values.push(Number(token))
    else return null
  }
  const vertices: RoutePoint[] = []
  for (let i = 0; i < commands.length; i += 1) {
    const { op, values } = commands[i]
    if (op === 'M' || op === 'L') {
      if (values.length !== 2) return null
      if (op === 'L' && commands[i + 1]?.op === 'Q') continue
      vertices.push({ x: values[0], y: values[1] })
    } else if (op === 'Q') {
      if (values.length !== 4) return null
      vertices.push({ x: values[0], y: values[1] })
    } else {
      return null
    }
  }
  if (vertices.length < 2 || !vertices.every(isRoutePoint)) return null
  return simplify(vertices)
}

// ── Settling control points ────────────────────────────────────────────────────

interface Handle {
  at: RoutePoint
  /** Unit vector pointing out of the node. */
  dir: RoutePoint
  /** Where the handle's straight run ends. */
  stub: RoutePoint
}

function handlesOf(ends: EdgeEnds, offset: number): { source: Handle; target: Handle } {
  const make = (at: RoutePoint, dir: RoutePoint): Handle => ({
    at,
    dir,
    stub: { x: at.x + dir.x * offset, y: at.y + dir.y * offset },
  })
  return {
    source: make(ends.source, UNIT[sideDir(ends.sourceSide, DOWN)]),
    target: make(ends.target, UNIT[sideDir(ends.targetSide, UP)]),
  }
}

interface Neighbour {
  point: RoutePoint
  /** Set when the neighbour is a handle's run end rather than another control point. */
  handle?: Handle
}

function alignTo(p: RoutePoint, neighbours: Neighbour[], tolerance: number): RoutePoint {
  const out = { ...p }
  for (const axis of ['x', 'y'] as const) {
    let best = tolerance
    let value = p[axis]
    let exact = false
    for (const n of neighbours) {
      // Taking a handle's row (or column) puts the point on the line straight
      // out of it: only from in front of the handle, never through its node.
      if (n.handle && n.handle.dir[axis] === 0) {
        const along = (p.x - n.handle.at.x) * n.handle.dir.x + (p.y - n.handle.at.y) * n.handle.dir.y
        if (along < 0) continue
      }
      const d = Math.abs(n.point[axis] - p[axis])
      // Already exactly on one neighbour's line: leave it there.
      if (d < 1e-6) {
        exact = true
        break
      }
      if (d < best) {
        best = d
        value = n.point[axis]
      }
    }
    if (!exact) out[axis] = value
  }
  return out
}

function clampOutOfRuns(p: RoutePoint, handles: Handle[], offset: number): RoutePoint {
  let q = p
  for (const h of handles) {
    // Only the square in front of the handle: a point off to the side at the
    // same depth is a choice about where a run goes, not a spike to avoid.
    const along = (q.x - h.at.x) * h.dir.x + (q.y - h.at.y) * h.dir.y
    const aside = Math.abs((q.x - h.at.x) * h.dir.y - (q.y - h.at.y) * h.dir.x)
    if (along >= 0 && along < offset && aside < offset) {
      q = { x: q.x + h.dir.x * (offset - along), y: q.y + h.dir.y * (offset - along) }
    }
  }
  return q
}

/**
 * Where a control point dropped at `p` actually goes: onto the row or column of
 * the point before or after it (`null` for the handle at that end) when within
 * `tolerance`, and never inside a handle's straight run — a point there can only
 * be reached by doubling back. Routing applies it with `JOG`; dragging applies it
 * too, so what is saved is exactly what is drawn.
 */
export function placeControlPoint(
  p: RoutePoint,
  prev: RoutePoint | null,
  next: RoutePoint | null,
  ends: EdgeEnds,
  tolerance = JOG,
  offset = EDGE_HANDLE_OFFSET,
): RoutePoint {
  const { source, target } = handlesOf(ends, offset)
  const neighbours: Neighbour[] = [
    prev ? { point: prev } : { point: source.stub, handle: source },
    next ? { point: next } : { point: target.stub, handle: target },
  ]
  // Pushing a point out of one handle's square can bring it within reach of a
  // row, or into the other handle's square: settle until it stays put, so
  // placing an already placed point leaves it where it is.
  let q = p
  for (let pass = 0; pass < 3; pass += 1) {
    const next = clampOutOfRuns(alignTo(q, neighbours, tolerance), [source, target], offset)
    if (next.x === q.x && next.y === q.y) break
    q = next
  }
  return q
}

// ── Choosing elbows ────────────────────────────────────────────────────────────

/** The corners a gap can turn at: horizontal-first then vertical-first, or none for a straight gap. */
function elbowCorners(a: RoutePoint, b: RoutePoint): Array<RoutePoint | null> {
  if (Math.abs(b.x - a.x) < EPS || Math.abs(b.y - a.y) < EPS) return [null]
  return [{ x: b.x, y: a.y }, { x: a.x, y: b.y }]
}

function rawLine(ends: EdgeEnds, anchors: RoutePoint[], corners: Array<RoutePoint | null>): RoutePoint[] {
  const line = [ends.source, anchors[0]]
  corners.forEach((corner, k) => {
    if (corner) line.push(corner)
    line.push(anchors[k + 1])
  })
  line.push(ends.target)
  return line
}

function scoreLine(raw: RoutePoint[], stubs: RoutePoint[]): number {
  const pts = simplify(raw)
  const heads: Array<Dir | null> = []
  const lengths: number[] = []
  for (let i = 0; i < pts.length - 1; i += 1) {
    heads.push(headingOf(pts[i], pts[i + 1]))
    lengths.push(dist(pts[i], pts[i + 1]))
  }
  let cost = 0
  for (let i = 1; i < heads.length; i += 1) {
    const a = heads[i - 1]
    const b = heads[i]
    if (a === null || b === null || a === b) continue
    if (isOpposite(a, b)) {
      cost += REVERSAL
      if (stubs.some(s => near(s, pts[i]))) cost += STUB_REVERSAL
    } else {
      cost += BEND
      if (stubs.some(s => near(s, pts[i]))) cost += STUB_BEND
    }
  }
  for (let i = 1; i + 1 < heads.length; i += 1) {
    const a = heads[i - 1]
    const b = heads[i]
    const c = heads[i + 1]
    if (a === null || b === null || c === null || a === b || isOpposite(a, b)) continue
    if (isOpposite(a, c) && lengths[i] < HAIRPIN_RUN && lengths[i - 1] >= JOG && lengths[i + 1] >= JOG) {
      cost += HAIRPIN
    }
  }
  return cost
}

/**
 * Last resort between otherwise equal lines: carry on before turning, earlier
 * gaps first. The weights halve per gap, so all of them together stay under a
 * single STUB_BEND.
 */
function tieBreak(anchors: RoutePoint[], corners: Array<RoutePoint | null>, out: Dir): number {
  let heading = out
  let cost = 0
  corners.forEach((corner, k) => {
    const a = anchors[k]
    const b = anchors[k + 1]
    if (corner) {
      const horizontalFirst = Math.abs(corner.y - a.y) < EPS
      const arrivingHorizontal = heading === RIGHT || heading === LEFT
      if (horizontalFirst !== arrivingHorizontal) cost += 0.1 * 2 ** -(k + 1)
      heading = headingOf(corner, b) ?? heading
    } else {
      heading = headingOf(a, b) ?? heading
    }
  })
  return cost
}

function chooseCorners(ends: EdgeEnds, anchors: RoutePoint[], out: Dir, into: Dir): Array<RoutePoint | null> {
  const options = anchors.slice(0, -1).map((a, k) => elbowCorners(a, anchors[k + 1]))
  const forks = options.flatMap((o, k) => (o.length > 1 ? [k] : []))
  if (forks.length > MAX_EXHAUSTIVE_FORKS) return chooseCornersGapByGap(anchors, out, into)
  const stubs = [anchors[0], anchors[anchors.length - 1]]
  let best = Infinity
  let bestCorners = options.map(o => o[0])
  for (let mask = 0; mask < 1 << forks.length; mask += 1) {
    const corners = options.map(o => o[0])
    forks.forEach((k, bit) => { corners[k] = options[k][(mask >> bit) & 1] })
    const cost = scoreLine(rawLine(ends, anchors, corners), stubs) + tieBreak(anchors, corners, out)
    if (cost < best - 1e-9) {
      best = cost
      bestCorners = corners
    }
  }
  return bestCorners
}

function turnCost(from: Dir, to: Dir): number {
  if (from === to) return 0
  return isOpposite(from, to) ? REVERSAL : BEND
}

/**
 * For lines with more control points than are worth scoring exhaustively: a
 * dynamic programme over the heading the line arrives in, with the same bend,
 * reversal and stub costs (but blind to U-turns, which span gaps).
 */
function chooseCornersGapByGap(anchors: RoutePoint[], out: Dir, into: Dir): Array<RoutePoint | null> {
  interface Step { from: Dir; corner: RoutePoint | null }
  let cost = [Infinity, Infinity, Infinity, Infinity]
  cost[out] = 0
  const trail: Array<Array<Step | null>> = []
  for (let k = 0; k < anchors.length - 1; k += 1) {
    const a = anchors[k]
    const b = anchors[k + 1]
    const next = [Infinity, Infinity, Infinity, Infinity]
    const via: Array<Step | null> = [null, null, null, null]
    for (const d of DIRS) {
      if (!Number.isFinite(cost[d])) continue
      for (const corner of elbowCorners(a, b)) {
        const moves = (corner ? [headingOf(a, corner), headingOf(corner, b)] : [headingOf(a, b)])
          .filter((m): m is Dir => m !== null)
        let c = cost[d]
        if (corner) c += tieBreak([a, b], [corner], d) * 2 ** -k
        if (k === 0 && moves.length > 0 && moves[0] !== d) c += STUB_BEND
        let heading = d
        for (const move of moves) {
          c += turnCost(heading, move)
          heading = move
        }
        if (c < next[heading]) {
          next[heading] = c
          via[heading] = { from: d, corner }
        }
      }
    }
    trail.push(via)
    cost = next
  }
  let heading: Dir = into
  let best = Infinity
  for (const d of DIRS) {
    const c = cost[d] + turnCost(d, into) + (d === into ? 0 : STUB_BEND)
    if (c < best) {
      best = c
      heading = d
    }
  }
  const corners: Array<RoutePoint | null> = new Array(trail.length).fill(null)
  for (let k = trail.length - 1; k >= 0; k -= 1) {
    const step = trail[k][heading]
    if (!step) break
    corners[k] = step.corner
    heading = step.from
  }
  return corners
}

// ── Ironing out jogs ───────────────────────────────────────────────────────────

function reversalsIn(pts: RoutePoint[]): number {
  let count = 0
  for (let i = 1; i < pts.length - 1; i += 1) {
    const a = headingOf(pts[i - 1], pts[i])
    const b = headingOf(pts[i], pts[i + 1])
    if (a !== null && b !== null && isOpposite(a, b)) count += 1
  }
  return count
}

function acceptableShift(candidate: RoutePoint[], original: RoutePoint[], waypoints: RoutePoint[]): boolean {
  if (candidate.length < 2) return false
  for (let i = 0; i < candidate.length - 1; i += 1) {
    if (headingOf(candidate[i], candidate[i + 1]) === null) return false
  }
  if (headingOf(candidate[0], candidate[1]) !== headingOf(original[0], original[1])) return false
  const n = candidate.length
  const m = original.length
  if (headingOf(candidate[n - 2], candidate[n - 1]) !== headingOf(original[m - 2], original[m - 1])) return false
  if (reversalsIn(candidate) > reversalsIn(original)) return false
  return waypoints.every(w => project(candidate, w).distance <= JOG + EPS)
}

/**
 * Iron out steps shorter than `JOG` between two runs heading the same way, by
 * lining one of those runs up with the other — the step a control point a few
 * pixels off its neighbour's row draws when the neighbour moved (a node nudged
 * since). The runs out of the source and into the target keep their direction,
 * a shift may not slant a run or add a reversal, and no control point may end
 * up more than `JOG` off the line; otherwise the step stays.
 */
function absorbJogs(points: RoutePoint[], waypoints: RoutePoint[]): RoutePoint[] {
  let pts = points
  for (let guard = 0; guard < points.length; guard += 1) {
    let next: RoutePoint[] | null = null
    // Segment j runs pts[j] → pts[j + 1]; the first (0) and last (n - 2) are the handles' runs.
    for (let j = 1; j < pts.length - 2 && !next; j += 1) {
      const jog = { x: pts[j + 1].x - pts[j].x, y: pts[j + 1].y - pts[j].y }
      if (Math.hypot(jog.x, jog.y) >= JOG) continue
      const before = headingOf(pts[j - 1], pts[j])
      if (before === null || before !== headingOf(pts[j + 1], pts[j + 2])) continue

      const candidates: Array<{ line: RoutePoint[]; length: number }> = []
      // Shift the run before the step onto the run after it…
      if (j - 1 >= 1) {
        const moved = [...pts]
        moved[j - 1] = { x: pts[j - 1].x + jog.x, y: pts[j - 1].y + jog.y }
        moved[j] = { ...pts[j + 1] }
        candidates.push({ line: simplify(moved), length: dist(pts[j - 1], pts[j]) })
      }
      // …or the run after it back onto the run before.
      if (j + 1 <= pts.length - 3) {
        const moved = [...pts]
        moved[j + 1] = { ...pts[j] }
        moved[j + 2] = { x: pts[j + 2].x - jog.x, y: pts[j + 2].y - jog.y }
        candidates.push({ line: simplify(moved), length: dist(pts[j + 1], pts[j + 2]) })
      }
      // Moving the shorter run changes the drawing least.
      const ok = candidates
        .filter(c => acceptableShift(c.line, pts, waypoints))
        .sort((a, b) => a.length - b.length)
      if (ok.length > 0) next = ok[0].line
    }
    if (!next) break
    pts = next
  }
  return pts
}

// ── Routing ────────────────────────────────────────────────────────────────────

export function routeOrthogonal(opts: EdgeEnds & { waypoints: RoutePoint[]; offset?: number }): OrthogonalRoute {
  const offset = opts.offset ?? EDGE_HANDLE_OFFSET
  const ends: EdgeEnds = { source: opts.source, sourceSide: opts.sourceSide, target: opts.target, targetSide: opts.targetSide }
  const { source, target } = handlesOf(ends, offset)
  const out = sideDir(ends.sourceSide, DOWN)
  const into = ((sideDir(ends.targetSide, UP) + 2) % 4) as Dir

  // Each point settles against the one before it as settled and the one after it
  // as it stands, so repeat until nothing moves: routing the settled points
  // again then settles them to themselves, and draws the same line.
  const settle = (tolerance: number) => {
    let current = opts.waypoints
    for (let pass = 0; pass < 3; pass += 1) {
      const settled: RoutePoint[] = []
      current.forEach((w, i) => {
        const prev = i > 0 ? settled[i - 1] : null
        const next = i < current.length - 1 ? current[i + 1] : null
        settled.push(placeControlPoint(w, prev, next, ends, tolerance, offset))
      })
      const moved = settled.some((w, i) => w.x !== current[i].x || w.y !== current[i].y)
      current = settled
      if (!moved) break
    }
    return current
  }
  const build = (waypoints: RoutePoint[]): OrthogonalRoute => {
    const anchors = [source.stub, ...waypoints, target.stub]
    const corners = chooseCorners(ends, anchors, out, into)
    const legs = corners.map((corner, k) => {
      const leg = [anchors[k]]
      if (corner) leg.push(corner)
      leg.push(anchors[k + 1])
      return leg
    })
    legs[0].unshift(ends.source)
    legs[legs.length - 1].push(ends.target)
    return { points: absorbJogs(simplify(legs.flat()), waypoints), legs, corners, waypoints }
  }

  const aligned = build(settle(JOG))
  // Lining points up is cosmetic: where it would make the line double back (a
  // point behind a node, pulled onto the row through it), route them as placed.
  const doubling = reversalsIn(aligned.points)
  if (doubling === 0) return aligned
  const asPlaced = build(settle(0))
  return reversalsIn(asPlaced.points) < doubling ? asPlaced : aligned
}

// ── Keeping an edge's shape while it is edited ─────────────────────────────────

interface Pin {
  point: RoutePoint
  arc: number
  /** Index of the line vertex this pin sits on, or -1 for a point that must stay. */
  vertex: number
}

/** How far apart two drawings of a line are: the furthest any point of one lies from the other. */
function deviation(a: RoutePoint[], b: RoutePoint[]): number {
  const sample = (line: RoutePoint[]) => {
    const out: RoutePoint[] = []
    for (let i = 0; i < line.length - 1; i += 1) {
      const steps = Math.max(1, Math.ceil(dist(line[i], line[i + 1]) / 4))
      for (let k = 0; k < steps; k += 1) {
        out.push({
          x: line[i].x + (line[i + 1].x - line[i].x) * k / steps,
          y: line[i].y + (line[i + 1].y - line[i].y) * k / steps,
        })
      }
    }
    if (line.length > 0) out.push(line[line.length - 1])
    return out
  }
  let worst = 0
  for (const p of sample(a)) worst = Math.max(worst, project(b, p).distance)
  for (const p of sample(b)) worst = Math.max(worst, project(a, p).distance)
  return worst
}

/**
 * Control points that make `routeOrthogonal` draw `line`, always including
 * `keep` (points on the line, in any order). Starts from every vertex of the
 * line and drops each one the route doesn't need — and, when `stable` is given,
 * only if what's left still passes it. If even all of them can't reproduce the
 * line exactly, reproduces what they do draw.
 */
function pinsFor(
  ends: EdgeEnds,
  line: RoutePoint[],
  keep: RoutePoint[],
  stable?: (pins: Pin[], goal: RoutePoint[]) => boolean,
): Pin[] {
  const arcs = [0]
  for (let i = 0; i < line.length - 1; i += 1) arcs.push(arcs[i] + dist(line[i], line[i + 1]))
  const pins: Pin[] = [
    ...line.slice(1, -1).map((point, i) => ({ point, arc: arcs[i + 1], vertex: i + 1 })),
    ...keep.map(p => {
      const hit = project(line, p)
      return { point: hit.point, arc: hit.arc, vertex: -1 }
    }),
  ].sort((a, b) => a.arc - b.arc || a.vertex - b.vertex)

  const draw = (list: Pin[]) => routeOrthogonal({ ...ends, waypoints: list.map(p => p.point) }).points
  const full = draw(pins)
  const goal = sameLine(full, line) ? line : full
  let list = pins
  for (let i = 0; i < list.length;) {
    if (list[i].vertex < 0) {
      i += 1
      continue
    }
    const without = list.filter((_, j) => j !== i)
    if (sameLine(draw(without), goal) && (!stable || stable(without, goal))) list = without
    else i += 1
  }
  return list
}

/**
 * A `pinsFor` check for the one kept point: sliding it a few pixels either way
 * along the run it sits on must not change the line. Without it a minimal set
 * can reproduce the line only by a whisker — a tie the next pixel of a drag
 * breaks, flipping a whole run to the other side.
 */
function slidesFreely(ends: EdgeEnds, line: RoutePoint[], kept: RoutePoint) {
  const hit = project(line, kept)
  const run = headingOf(line[hit.segment], line[hit.segment + 1])
  if (run === null) return undefined
  const along = run === RIGHT || run === LEFT ? 'x' : 'y'
  const a = line[hit.segment][along]
  const b = line[hit.segment + 1][along]
  const slides = [-10, -3, 3, 10]
    .map(d => hit.point[along] + d)
    .filter(v => v > Math.min(a, b) + EPS && v < Math.max(a, b) - EPS)
  return (pins: Pin[], goal: RoutePoint[]) => {
    const index = pins.findIndex(pin => pin.vertex < 0)
    return slides.every(v => {
      const points = pins.map(pin => pin.point)
      points[index] = along === 'x' ? { x: v, y: points[index].y } : { x: points[index].x, y: v }
      return sameLine(routeOrthogonal({ ...ends, waypoints: points }).points, goal)
    })
  }
}

export interface DragPlan {
  /** Control points the drag starts from. */
  points: RoutePoint[]
  /** The point under the pointer. */
  index: number
  /**
   * Points that bound the run the grabbed point sits on. They move with it
   * across the run (`across` is the axis they follow), so dragging the middle
   * of a run shifts the whole run instead of bending it.
   */
  follow: number[]
  across: 'x' | 'y' | null
  /** How far the grabbed point may slide along its run: up to the bounds that are pinned. */
  slide: { axis: 'x' | 'y'; min: number; max: number } | null
}

/**
 * Turn the handle drawn in the middle of an edge without control points into
 * real ones, without changing the line: `centre` plus whichever corners of the
 * default path the route needs to draw it the same way, and keep drawing it the
 * same way while the handle slides along its run.
 */
export function planImplicitDrag(ends: EdgeEnds, line: RoutePoint[] | null, centre: RoutePoint): DragPlan {
  if (!line || line.length < 2) return { points: [{ ...centre }], index: 0, follow: [], across: null, slide: null }
  const hit = project(line, centre)
  const pins = pinsFor(ends, line, [hit.point], slidesFreely(ends, line, hit.point))
  const run = headingOf(line[hit.segment], line[hit.segment + 1])
  const bounds = [hit.segment, hit.segment + 1].filter(v => v > 0 && v < line.length - 1)
  const follow = pins.flatMap((p, i) => (bounds.includes(p.vertex) ? [i] : []))
  let slide: DragPlan['slide'] = null
  if (run !== null && follow.length > 0) {
    const axis = run === RIGHT || run === LEFT ? 'x' : 'y'
    const at = hit.point[axis]
    let min = -Infinity
    let max = Infinity
    for (const i of follow) {
      const v = pins[i].point[axis]
      if (v < at) min = Math.max(min, v + 1)
      else max = Math.min(max, v - 1)
    }
    slide = { axis, min, max }
  }
  return {
    points: pins.map(p => p.point),
    index: pins.findIndex(p => p.vertex < 0),
    follow,
    across: run === null ? null : (run === RIGHT || run === LEFT ? 'y' : 'x'),
    slide,
  }
}

/**
 * Where on `line` a new control point goes, as close to `hit` as allowed: a
 * handle's straight run away from either end, outside the square in front of
 * each handle (settling would push it off the line), and more than `JOG` from
 * the control points already there (settling would pull one onto it).
 */
function spotOnLine(ends: EdgeEnds, line: RoutePoint[], hit: LineHit, others: RoutePoint[], offset: number): RoutePoint {
  const total = lineLength(line)
  const lo = Math.min(offset, total / 2)
  const hi = Math.max(total - offset, total / 2)
  const start = Math.min(Math.max(hit.arc, lo), hi)
  const { source, target } = handlesOf(ends, offset)
  const fits = (q: RoutePoint) => {
    const out = clampOutOfRuns(q, [source, target], offset)
    return out.x === q.x && out.y === q.y && others.every(o => dist(o, q) > JOG + EPS)
  }
  const at = (arc: number) => (arc === hit.arc ? hit.point : pointAtArc(line, arc))
  for (let step = 0; step <= 4 * offset; step += 1) {
    for (const arc of step === 0 ? [start] : [start - step, start + step]) {
      if (arc < lo || arc > hi) continue
      const q = at(arc)
      if (fits(q)) return q
    }
  }
  return at(start)
}

/**
 * Control points after adding one where the user double-clicked (`p`), chosen
 * so the line keeps its shape: the new point goes onto the line as drawn —
 * `defaultLine` when there are no control points yet — and whatever else the
 * route needs to keep drawing that line is pinned alongside it. Of the lists
 * tried, the first that reproduces the line wins, else the one closest to it.
 */
export function insertControlPoint(
  ends: EdgeEnds,
  points: RoutePoint[],
  defaultLine: RoutePoint[] | null,
  p: RoutePoint,
  offset = EDGE_HANDLE_OFFSET,
): RoutePoint[] {
  if (points.length === 0) {
    if (!defaultLine || defaultLine.length < 2) return [{ ...p }]
    const q = spotOnLine(ends, defaultLine, project(defaultLine, p), [], offset)
    return pinsFor(ends, defaultLine, [q], slidesFreely(ends, defaultLine, q)).map(pin => pin.point)
  }

  const before = routeOrthogonal({ ...ends, waypoints: points, offset })
  // Build on the points as routed, so none of them settles anew beside the new one.
  const base = before.waypoints
  const q = spotOnLine(ends, before.points, project(before.points, p), base, offset)
  let leg = 0
  let nearest = Infinity
  before.legs.forEach((legPoints, index) => {
    const distance = project(legPoints, q).distance
    if (distance < nearest) {
      nearest = distance
      leg = index
    }
  })
  const splice = (added: RoutePoint[]) => [...base.slice(0, leg), ...added, ...base.slice(leg)]
  const corner = before.corners[leg]
  const candidates = [splice([q])]
  if (corner) candidates.push(splice([q, { ...corner }]), splice([{ ...corner }, q]))

  let best = candidates[0]
  let bestDeviation = Infinity
  for (const candidate of candidates) {
    const drawn = routeOrthogonal({ ...ends, waypoints: candidate, offset }).points
    if (sameLine(drawn, before.points)) return candidate
    const d = deviation(drawn, before.points)
    if (d < bestDeviation) {
      bestDeviation = d
      best = candidate
    }
  }
  // Last resort: keep every point and pin the line's corners around them —
  // possible only while the points still run along the line in order.
  const arcs = [...base, q].map(point => project(before.points, point).arc)
  const inOrder = base.every((_, i) => i === 0 || arcs[i] >= arcs[i - 1])
  if (inOrder) {
    const pinned = pinsFor(ends, before.points, [...base, q]).map(pin => pin.point)
    const drawn = routeOrthogonal({ ...ends, waypoints: pinned, offset }).points
    if (deviation(drawn, before.points) < bestDeviation) return pinned
  }
  return best
}
