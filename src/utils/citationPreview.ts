/**
 * The bibliography entry a citation link points at, read off the PDF's own text — what the
 * reader's hover preview shows, so checking "(Li et al., 2025)" does not mean scrolling to the
 * references and back.
 *
 * A link only knows a destination: a page and a point (hyperref puts `cite.<key>` at the entry's
 * label, raised by about one line). Everything else comes from the page geometry pdf.js reports
 * (`furniturePageFromTextContent`, scale-1 CSS px, top-left origin):
 *
 * - The entry starts on the first line whose baseline lies at or just below the point, at the
 *   span next to the point's x.
 * - Its column is the x range from that span to the next gutter: an x where many lines of the
 *   page agree on a gap (the second column, margin line numbers). Interword gaps of a badly
 *   stretched line do not line up, so they never pass for one.
 * - It ends where the next destination on the page begins (every entry BibTeX prints is cited,
 *   so every entry carries one). An entry that runs off the bottom of its column is continued
 *   from above the next destination of the same family (`cite.`, `bm_CR`, a pax prefix…), in the
 *   next column or on the next page.
 * - Without such boundaries (explicit destinations, the last entry), the layout decides: a gap
 *   taller than the column's leading, a new label like the first one (`[12]`, `12.`), a line
 *   flush with the first after hanging-indented ones, a larger heading.
 *
 * Pure and synchronous: the viewer fetches the destinations and text content and passes them in.
 */
import type { FurniturePage, FurnitureSpan } from './pageFurniture'
import { mergeWrappedLines } from './highlightText'

export interface RefDestination {
  /** 0-based page index. */
  page: number
  /** Scale-1 CSS px from the page's left edge, or null when the destination does not say. */
  x: number | null
  /** Scale-1 CSS px from the page's top edge, or null when the destination does not say. */
  y: number | null
  /** Named destinations only (hyperref's `cite.<key>`, Springer's `bm_CR12`, …). */
  name?: string
}

export interface ReferenceLookup {
  target: RefDestination
  /** The named destinations of the target page and of the next one (any order, any family). */
  destinations: readonly RefDestination[]
  /** Text geometry of the target page and, when the entry may continue there, the next page. */
  pages: ReadonlyMap<number, FurniturePage>
}

export interface ReferenceEntry {
  text: string
  /** True when a boundary destination closed the entry, false when only the layout did. */
  bounded: boolean
}

/** pdf.js places a span's top this far above its baseline (pageFurniture's ASCENT_RATIO). */
const ASCENT = 0.8
/** Every line of an entry starts at its column's edge, give or take a hanging indent or a label
 *  (em). See `outOfColumn`. */
const MAX_INDENT = 4.5
const MAX_TEXT = 4000

// Families of destinations that never name a bibliography entry: hyperref's counters, and the
// figure / table / section anchors publishers write (`bm_Fig3`, `fig9005`, `tbl2`).
const NON_REFERENCE_FAMILY = new RegExp(
  '^(?:figure|fig|table|tbl|tab|(?:sub)*section|sec|paragraph|chapter|part|appendix|app|page|'
  + 'equation|eqn?|align|h?footnote|fn|item|enumi+|theorem|lemma|corollary|proposition|definition|'
  + 'remark|example|assumption|claim|conjecture|hypothesis|observation|fact|note|problem|exercise|'
  + 'question|algorithm|algocf|alg@line|algoline|lstlisting|lstnumber|listing|subfigure|subtable|'
  + 'ams|cfigure|ctable|doc-start|toc|thmt@dummyctr|alc@unique|bm_(?:fig|tab|sec|equ|aff|app|fn))'
  + '(?![a-z])', 'i')

/**
 * What kind of destination a link names: `'cite.'` for hyperref's bibliography anchors, a family
 * shared by sibling destinations (the name without its trailing number: `bm_CR`, `ref`, a pax
 * prefix), `null` when nothing is known (explicit destinations, names without a number) and
 * `false` for families that are figures, sections, equations, pages, … and never references.
 */
export function destinationFamily(name: string | undefined): string | null | false {
  if (!name) return null
  if (name.startsWith('cite.')) return 'cite.'
  if (NON_REFERENCE_FAMILY.test(name)) return false
  const stem = name.replace(/\d+$/, '')
  return stem && stem !== name ? stem : null
}

/**
 * Whether text read at a destination reads like a bibliography entry rather than a section, a
 * figure or an equation. Only links whose destination family does not already say so need it
 * (together with `referencesHeading`: body text can quote years and "et al." too).
 */
export function looksLikeReference(text: string): boolean {
  if (text.length < 25 || text.length > MAX_TEXT + 1) return false
  if (/^(?:fig(?:ure)?s?\b|tab(?:le)?s?\b|algorithm|section|sec\.|appendix|theorem|lemma|definition|proof|eq(?:uation)?s?\b|corollary|proposition|remark|example|step\b|abstract\b)/i.test(text)) return false
  // Numbered headings ("4.1 Baseline", "A.2 Setup", "B Proofs"); a numbered entry has "4." instead.
  if (/^(?:[A-Z]|\d+)(?:\.\d+)+\s|^[A-Z]\s+\p{Lu}\p{Ll}/u.test(text)) return false
  if (!/(?:^|\D)(?:1[5-9]\d\d|20\d\d)[a-z]?(?!\d)/.test(text)) return false
  if (/\bet al\b|\barxiv\b|\bcorr\b|\bproc(?:eedings|\.)|\bjournal\b|\bconference\b|\bpp?\.\s*\d|\bvol\.|\bdoi\b|https?:\/\/|\bin:\s|\bpress\b|\buniversit|\btrans(?:actions|\.)|\bpreprint\b|\blett\.|\brev\.|\bed(?:s|itors?)?\.?\)/i.test(text)) return true
  // Author lists ("Surname, A., Surname, B."), names then the year ("John Cocke. 1969.",
  // "Kim, N. & Linzen, T. (2020)"), Vancouver's "Bioinform 2022;23:" and Nature's "14, 1–9 (2011).".
  const body = text.replace(LABEL, '').trim()
  return (body.match(/,/g)?.length ?? 0) >= 3
    || /^[^\d]{2,300}?[.,(]\s*\(?(?:1[5-9]\d\d|20\d\d)[a-z]?\)?[.,:]/.test(body)
    || /\s(?:1[5-9]\d\d|20\d\d)[a-z]?;\s?\d/.test(body)
    || /\(\s*(?:1[5-9]\d\d|20\d\d)[a-z]?\s*\)\.?$/.test(body)
}

/**
 * False when the link names a number ("12", "[12]") and the entry found starts with another
 * label: the destination's point was too vague (a box as wide as the page) and the wrong entry
 * came back. Better no preview than someone else's reference.
 */
export function labelAgrees(linkText: string, entry: string): boolean {
  const cited = /^\[?\s*(\d{1,4})\s*\]?[,;.]?$/.exec(linkText.trim())
  const found = /^[[(]?(\d{1,4})[.\])]/.exec(entry)
  return !cited || !found || cited[1] === found[1]
}

const REFERENCES_HEADING = /^(?:\d+\.?|[A-Z]\.?|[IVX]+\.?)?(?:references?|bibliography|literaturecited|workscited|citedliterature|referencesandnotes|参考文献)$/i

/**
 * Whether a page carries the heading of a reference list. A link outside the `cite.` family only
 * previews text on or after that page: before it, the same years and "et al." are body text.
 * Small caps come apart into spans ("R" "EFERENCES"), so a line's runs are joined first.
 */
export function referencesHeading(page: FurniturePage): boolean {
  for (const line of groupLines(usable(page))) {
    const ordered = [...line.spans].sort((a, b) => a.x - b.x)
    let run = ''
    let prevEnd = -Infinity
    for (const s of ordered) {
      if (run && s.x - prevEnd > 1.5 * s.fontSize) {
        if (REFERENCES_HEADING.test(run)) return true
        run = ''
      }
      run += s.text.replace(/\s+/g, '')
      prevEnd = s.x + s.w
    }
    if (REFERENCES_HEADING.test(run)) return true
  }
  return false
}

/**
 * Where an explicit destination (`[page, {name: fit}, …]`) points on a page with `view`
 * (`page.view`), in the coordinates `furniturePageFromTextContent` uses. A `FitR` box as wide as
 * the page (Springer's) says nothing about the column, so it has no x; its top is the larger of its
 * two y values (producers write them either way round).
 */
export function destinationPoint(dest: readonly unknown[], view: readonly number[]): { x: number | null; y: number | null } {
  const fit = (dest[1] as { name?: string } | null)?.name
  const num = (i: number) => (typeof dest[i] === 'number' ? dest[i] as number : null)
  let left: number | null = null
  let top: number | null = null
  switch (fit) {
    case 'XYZ': left = num(2); top = num(3); break
    case 'FitH': case 'FitBH': top = num(2); break
    case 'FitV': case 'FitBV': left = num(2); break
    case 'FitR': {
      const [l, b, r, t] = [num(2), num(3), num(4), num(5)]
      if (b !== null && t !== null) top = Math.max(b, t)
      if (l !== null && r !== null && Math.abs(r - l) < 0.6 * (view[2] - view[0])) left = Math.min(l, r)
      break
    }
    default: break
  }
  return {
    x: left === null ? null : left - view[0],
    y: top === null ? null : view[3] - top,
  }
}

interface Line {
  baseline: number
  /** Size of the line's longest span: the text's own size, not a superscript's. */
  fs: number
  spans: FurnitureSpan[]
  x0: number
  x1: number
}

const baselineOf = (s: FurnitureSpan) => s.y + s.fontSize * ASCENT
const centerX = (s: FurnitureSpan) => s.x + s.w / 2

function usable(page: FurniturePage): FurnitureSpan[] {
  return page.spans.filter(s => !s.angle && s.fontSize > 0 && s.text.trim() !== '')
}

/** Spans grouped into lines by baseline, top to bottom. */
function groupLines(spans: readonly FurnitureSpan[]): Line[] {
  const sorted = [...spans].sort((a, b) => baselineOf(a) - baselineOf(b))
  const lines: Line[] = []
  let anchor: FurnitureSpan | null = null
  for (const s of sorted) {
    const b = baselineOf(s)
    const last = lines[lines.length - 1]
    if (last && anchor && b - baselineOf(anchor) <= 0.45 * Math.max(anchor.fontSize, s.fontSize)) {
      last.spans.push(s)
      // The line's own text anchors it, so sub/superscripts and accents drawn as separate glyphs
      // (which sit on baselines of their own) join it instead of starting lines of their own.
      if (s.text.trim().length > anchor.text.trim().length) anchor = s
      continue
    }
    lines.push({ baseline: b, fs: s.fontSize, spans: [s], x0: 0, x1: 0 })
    anchor = s
  }
  for (const line of lines) {
    let longest = line.spans[0]
    for (const s of line.spans) if (s.text.trim().length > longest.text.trim().length) longest = s
    line.baseline = baselineOf(longest)
    line.fs = longest.fontSize
    line.x0 = Math.min(...line.spans.map(s => s.x))
    line.x1 = Math.max(...line.spans.map(s => s.x + s.w))
  }
  return lines
}

// Spacing accents a font without precomposed glyphs draws over the letter (OT1 `\"o`), and the
// combining mark each stands for.
const SPACING_ACCENTS: Record<string, string> = {
  '¨': '̈', '´': '́', 'ˆ': '̂', '˜': '̃', '¸': '̧',
  'ˇ': '̌', '˘': '̆', '˚': '̊', '¯': '̄', '˙': '̇',
  '˝': '̋', '˛': '̨',
}
const ACCENT_ONLY = /^[¨´ˆ˜¸ˇ˘˚¯˙˝˛]$/

/** One line's text, spans in x order with a space where the gap between them is a word gap. */
function lineText(line: Line): string {
  // An accent drawn as its own glyph sits over its letter. Sorted a little left, it lands before
  // the letter when that begins the next span (OT1 `\"o`), and it is folded into the previous
  // character when it sits over the end of the previous span (`\AA`: A, then the ring).
  const key = (s: FurnitureSpan) => ACCENT_ONLY.test(s.text.trim()) ? s.x - 0.3 * s.fontSize : s.x
  const ordered = [...line.spans].sort((a, b) => key(a) - key(b))
  let out = ''
  let prevEnd = -Infinity
  for (const s of ordered) {
    const accent = ACCENT_ONLY.test(s.text.trim()) ? s.text.trim() : null
    if (accent && s.x + s.w / 2 < prevEnd && /[A-Za-z]$/.test(out)) {
      out = out.slice(0, -1) + (out.slice(-1) + SPACING_ACCENTS[accent]).normalize('NFC')
      continue
    }
    if (out && !accent && !/\s$/.test(out) && !/^\s/.test(s.text)
      && !ACCENT_ONLY.test(out.slice(-1)) && s.x - prevEnd > 0.15 * s.fontSize) out += ' '
    out += accent ?? s.text
    prevEnd = Math.max(prevEnd, s.x + s.w)
  }
  return composeAccents(out.replace(/\s+/g, ' ').trim())
}

function composeAccents(text: string): string {
  return text.replace(
    /([\u00a8\u00b4\u02c6\u02dc\u00b8\u02c7\u02d8\u02da\u00af\u02d9\u02dd\u02db])\s?([A-Za-z\u0131])/g,
    (_m, accent: string, letter: string) => ((letter === '\u0131' ? 'i' : letter) + SPACING_ACCENTS[accent]).normalize('NFC'),
  )
}

/**
 * The lines as one paragraph. A URL or DOI cut at the end of a line goes on without a space, and a
 * word hyphenated across lines is rejoined when the page spells it whole elsewhere
 * ("Com-" + "putational" next to "Computational"); otherwise the hyphen stays, as in
 * `mergeWrappedLines` (a compound like "fine-tuned" must not lose it).
 */
function joinLines(lines: readonly string[], vocabulary: ReadonlySet<string>): string {
  const merged: string[] = []
  for (const line of lines) {
    const prev = merged[merged.length - 1]
    if (prev === undefined) { merged.push(line); continue }
    const lastToken = prev.slice(prev.lastIndexOf(' ') + 1)
    if (/^(?:https?:|www\.|doi:|10\.\d{4,}\/)/i.test(lastToken)
      && (/[/._\-=?&#%~:]$/.test(lastToken) || line.startsWith('//')) && !/^[\s([]/.test(line)) {
      merged[merged.length - 1] = prev + line
      continue
    }
    const broken = /(\p{L}+)-$/u.exec(prev)
    const rest = /^(\p{Ll}+)/u.exec(line)
    if (broken && rest && vocabulary.has((broken[1] + rest[1]).toLowerCase())
      && !vocabulary.has(`${broken[1]}-${rest[1]}`.toLowerCase())) {
      merged[merged.length - 1] = prev.slice(0, -1) + line
      continue
    }
    merged.push(line)
  }
  return mergeWrappedLines(merged.join('\n'))
}

/** Every word (and hyphenated compound) printed on the pages, lower-cased. */
function wordsOf(pages: Iterable<FurniturePage>): Set<string> {
  const words = new Set<string>()
  for (const page of pages) {
    for (const s of page.spans) {
      for (const w of s.text.toLowerCase().match(/\p{L}+(?:-\p{L}+)*/gu) ?? []) {
        words.add(w)
        if (w.includes('-')) for (const part of w.split('-')) words.add(part)
      }
    }
  }
  return words
}

/** Each span's position in the page's content stream (the order pdf.js lists them in). */
const streamOrder = new WeakMap<FurniturePage, Map<FurnitureSpan, number>>()
function orderOf(page: FurniturePage): Map<FurnitureSpan, number> {
  let order = streamOrder.get(page)
  if (!order) {
    order = new Map(page.spans.map((s, i) => [s, i]))
    streamOrder.set(page, order)
  }
  return order
}

/**
 * Where the column after the one starting at `left` begins: the first x right of it where lines
 * near `baseline` start, aligned, and that no line's text runs across — the gutter before the next
 * column, or margin line numbers.
 *
 * "Runs across" is judged by the content stream, not by widths: the words of one line follow each
 * other in the stream while two columns sharing a row are far apart in it. Widths are not to be
 * trusted for this (some fonts report italic runs a third too wide, into the next column), and
 * gaps are not either (some producers write every word as its own span, so a justified line is a
 * row of 1-em gaps).
 */
function nextColumnStart(page: FurniturePage, spans: readonly FurnitureSpan[], left: number, baseline: number, fs: number): number | null {
  const order = orderOf(page)
  const adjacent = (a: FurnitureSpan, b: FurnitureSpan) => Math.abs((order.get(a) ?? -9) - (order.get(b) ?? 9)) <= 2
  const lines = groupLines(spans.filter(s => {
    const b = baselineOf(s)
    return b > baseline - 30 * fs && b < baseline + 45 * fs && s.x + s.w > left
  })).map(line => [...line.spans].sort((a, b) => a.x - b.x))
  const minStart = left + Math.max(0.2 * page.width, 8 * fs)
  const starts: { x: number; line: number }[] = []
  lines.forEach((row, n) => {
    for (let k = 0; k < row.length; k++) {
      const s = row[k]
      const p = row[k - 1]
      if (s.x < minStart) continue
      if (!p || s.x - (p.x + p.w) >= 0.9 * s.fontSize || (!adjacent(p, s) && s.x - p.x > 2 * s.fontSize)) {
        starts.push({ x: s.x, line: n })
      }
    }
  })
  starts.sort((a, b) => a.x - b.x)
  for (let i = 0; i < starts.length;) {
    let j = i
    while (j < starts.length && starts[j].x - starts[i].x <= 0.6 * fs) j++
    const x = starts[i].x
    const aligned = new Set(starts.slice(i, j).map(s => s.line)).size
    if (aligned >= 3) {
      const edge = x - 0.3 * fs
      const crossing = lines.filter(row => row.some((s, k) => {
        const p = row[k - 1]
        return p !== undefined && p.x < edge && s.x >= edge && adjacent(p, s)
      })).length
      if (crossing <= Math.max(1, 0.2 * aligned)) return x
    }
    i = j
  }
  return null
}

interface Column {
  left: number
  right: number
  lines: Line[]
}

function columnAt(page: FurniturePage, spans: readonly FurnitureSpan[], left: number, baseline: number, fs: number): Column {
  const next = nextColumnStart(page, spans, left, baseline, fs)
  const right = next === null ? page.width : next - 0.3 * fs
  const lines = groupLines(spans.filter(s => {
    const c = centerX(s)
    return c >= left && c < right && s.x >= left - 0.5 * fs
  }))
  return { left, right, lines }
}

/** How far below its point an entry's baseline usually lies, in em: hyperref raises the anchor by
 *  about one line (measured on 10 703 `cite.` anchors: 0.9–1.4, mostly 1.1–1.2). */
const DEFAULT_RAISE = 1.15

const raiseOf = (s: FurnitureSpan, y: number) => (baselineOf(s) - y) / s.fontSize

/** Whether `s` continues the text of a span just left of it (the next word, a trailing comma). */
function continuesLine(page: FurniturePage, spans: readonly FurnitureSpan[], s: FurnitureSpan): boolean {
  const order = orderOf(page)
  const i = order.get(s) ?? -9
  return spans.some(p => p !== s && Math.abs((order.get(p) ?? 9) - i) <= 2 && p.x < s.x
    && s.x - (p.x + p.w) < s.fontSize && Math.abs(baselineOf(p) - baselineOf(s)) <= 0.45 * s.fontSize)
}

/** Spans that could start the entry at a point: line starts near it, at or after its x. */
function startCandidates(page: FurniturePage, spans: readonly FurnitureSpan[], x: number, y: number): FurnitureSpan[] {
  const near = spans.filter(s => {
    const r = raiseOf(s, y)
    return r >= -0.5 && r <= 2.6 && !continuesLine(page, spans, s)
  })
  // natbib's anchor sits in the label box, up to ~2.5 em left of the text and right on top of
  // margin line numbers, so a span merely covering the x only counts when no line has one
  // starting after it (a destination set mid-line).
  const after = near.filter(s => s.x >= x - 0.3 * s.fontSize && s.x - x <= 4 * s.fontSize)
  if (!after.length) return near.filter(s => s.x <= x && s.x + s.w >= x)
  // An entry starts further left than the lines around it: its continuation lines are indented
  // and the next entry's label or first word is no further left. Among the leftmost starts, the
  // raise decides (the anchor can sit on the previous entry's last baseline).
  const minX = Math.min(...after.map(s => s.x))
  return after.filter(s => s.x - minX <= 0.8 * s.fontSize)
}

/** The span an entry starts with: the leftmost of the candidate line whose raise is closest. */
function startSpan(page: FurniturePage, spans: readonly FurnitureSpan[], x: number | null, y: number, raise = DEFAULT_RAISE): FurnitureSpan | null {
  if (x === null) return startSpanInAnyColumn(page, spans, y, raise)
  let best: FurnitureSpan | null = null
  let bestDistance = Infinity
  for (const s of startCandidates(page, spans, x, y)) {
    const distance = Math.abs(raiseOf(s, y) - raise)
    const sameLine = best !== null
      && Math.abs(baselineOf(s) - baselineOf(best)) <= 0.45 * Math.max(s.fontSize, best.fontSize)
    if (sameLine ? s.x < best!.x : distance < bestDistance) { best = s; bestDistance = distance }
  }
  return best
}

/**
 * A destination without an x (`FitH`, `XYZ null`) does not say which column it means. Every
 * column's line near the point is a candidate; the raise picks among them, and a line indented
 * from its neighbours (an entry's continuation, not its start) loses to one that is not.
 */
function startSpanInAnyColumn(page: FurniturePage, spans: readonly FurnitureSpan[], y: number, raise: number): FurnitureSpan | null {
  const near = spans.filter(s => {
    const r = raiseOf(s, y)
    return r >= -0.5 && r <= 2.6
  })
  if (!near.length) return null
  const fs = median(near.map(s => s.fontSize))!
  const baseline = y + raise * fs
  let left = Math.min(...spans.filter(s => Math.abs(baselineOf(s) - baseline) < 30 * fs).map(s => s.x)) - 0.6 * fs
  let best: FurnitureSpan | null = null
  let bestScore = Infinity
  for (let n = 0; n < 6; n++) {
    const col = columnAt(page, spans, left, baseline, fs)
    // A sliver of a column holds margin line numbers, not entries.
    if (col.right - col.left > 0.15 * page.width) {
      for (let i = 0; i < col.lines.length; i++) {
        const line = col.lines[i]
        const r = (line.baseline - y) / line.fs
        if (r < -0.5 || r > 2.6) continue
        const around = col.lines.slice(Math.max(0, i - 3), i + 4)
        const indented = line.x0 - Math.min(...around.map(l => l.x0)) > 0.5 * line.fs && !LABEL.test(lineText(line))
        const score = Math.abs(r - raise) + (indented ? 0.6 : 0)
        if (score < bestScore) {
          bestScore = score
          best = line.spans.reduce((m, s) => (s.x < m.x ? s : m))
        }
      }
    }
    const next = nextColumnStart(page, spans, left, baseline, fs)
    if (next === null) break
    left = next - 0.6 * fs
  }
  return best
}

/**
 * The raise of the anchors on a page, from the siblings whose entry start is unambiguous (one
 * leftmost line near the point). Producers differ: hyperref raises by a line, others point at the
 * line's top or below its baseline.
 */
function pageRaise(page: FurniturePage, spans: readonly FurnitureSpan[], points: readonly RefDestination[]): number {
  const raises: number[] = []
  for (const d of points) {
    if (d.x === null || d.y === null) continue
    const c = startCandidates(page, spans, d.x, d.y)
    if (!c.length) continue
    const b = baselineOf(c[0])
    if (c.every(s => Math.abs(baselineOf(s) - b) <= 0.45 * s.fontSize)) raises.push(raiseOf(c[0], d.y))
  }
  return raises.length >= 3 ? median(raises)! : DEFAULT_RAISE
}

/** A label set apart just left of the entry's first span (`[12]` when the anchor follows it). */
function labelBefore(spans: readonly FurnitureSpan[], first: FurnitureSpan): FurnitureSpan | null {
  const b = baselineOf(first)
  let best: FurnitureSpan | null = null
  for (const s of spans) {
    const gap = first.x - (s.x + s.w)
    if (gap < -0.1 * first.fontSize || gap > 1.5 * first.fontSize) continue
    if (Math.abs(baselineOf(s) - b) > 0.45 * first.fontSize || !LABEL.test(s.text.trim())) continue
    if (!best || s.x > best.x) best = s
  }
  return best
}

const LABEL = /^(?:\[[^\]\s][^\]]{0,14}\]|\d{1,4}\.(?=\s)|\(\d{1,4}\))/

/** The label a line starts with, and its number when it is a plain one (`[12]`, `12.`, `(12)`). */
function labelOf(text: string): { n: number | null } | null {
  const m = LABEL.exec(text)
  if (!m) return null
  const n = /^[[(]?(\d+)[.\])]$/.exec(m[0])
  return { n: n ? Number(n[1]) : null }
}

function median(values: number[]): number | null {
  if (!values.length) return null
  const v = [...values].sort((a, b) => a - b)
  return v[Math.floor(v.length / 2)]
}

/** Reading order of an entry start: page, then left/right half, then top to bottom. */
function readingKey(pageIndex: number, page: FurniturePage, span: FurnitureSpan): [number, number, number] {
  return [pageIndex, centerX(span) > page.width / 2 ? 1 : 0, baselineOf(span)]
}

function before(a: [number, number, number], b: [number, number, number]): boolean {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i]
  return false
}

/**
 * A line that starts well inside the column, at an x no other line of the column starts at: a
 * centred running head or heading, not an entry's line. Continuation lines can sit far in too (after
 * labels like `[MKM+94]`), but they all start at the same x.
 */
function outOfColumn(col: Column, line: Line, edge: number, fs: number, head: Line): boolean {
  if (line.x0 - edge <= MAX_INDENT * fs) return false
  // Aligned with the text after the first line's label, or with enough other lines.
  if (head.spans.some(s => Math.abs(s.x - line.x0) <= 0.5 * fs)) return false
  return col.lines.filter(l => Math.abs(l.x0 - line.x0) <= 0.5 * fs).length < 3
}

/** A line that is only a number: a page number, or a line number in the margin. */
function bareNumber(line: Line): boolean {
  return /^\d{1,5}$/.test(lineText(line))
}

/** Set in the entry's size: a heading is larger, a page number or running head smaller. */
function sameType(line: Line, fs: number, tolerance?: number): boolean {
  if (tolerance !== undefined) return Math.abs(line.fs - fs) <= tolerance * fs
  return line.fs <= 1.12 * fs && line.fs >= 0.8 * fs
}

/**
 * Lines of the next column (or page) that precede `first`, the start of the next entry: the tail
 * of an entry that ran off the bottom of its own column. Walked upwards while the leading stays
 * even, so a running head or the column's top margin ends it. Lines found to belong to another
 * entry in between (one that starts further left, or stands apart by the space between entries)
 * are dropped: that entry's anchor slipped to the previous page or is missing.
 */
function continuationBefore(page: FurniturePage, first: FurnitureSpan, fs: number, leading: number): Line[] {
  const spans = usable(page)
  const col = columnAt(page, spans, (labelBefore(spans, first) ?? first).x - 0.6 * fs, baselineOf(first), fs)
  const at = col.lines.findIndex(l => l.spans.includes(first))
  let out: Line[] = []
  let below = col.lines[at]
  for (let i = at - 1; i >= 0 && out.length < 30; i--) {
    const line = col.lines[i]
    if (bareNumber(line)) continue
    const gap = below.baseline - line.baseline
    // The first step crosses the space between two entries (2.2 em in ICML's style), later ones
    // the leading; the column's top margin is wider. Only the entry's own size counts: above it
    // there may be a running head, a heading or body text (an appendix).
    const limit = out.length ? Math.max(2.2 * fs, 1.7 * leading) : Math.max(3 * fs, 2.2 * leading)
    if (gap > limit || !sameType(line, fs, 0.08) || outOfColumn(col, line, first.x, fs, col.lines[at])) break
    below = line
    if (out.length && line.x0 < Math.min(...out.map(l => l.x0)) - 0.5 * fs) { out = []; continue }
    if (out.length && gap > leading + 0.25 * fs) out = []
    out.unshift(line)
  }
  return out
}

/**
 * The entry at `lookup.target`, or null when no text starts there (a rotated page, a scan, an
 * image-only page, a point in empty space).
 */
export function findReferenceEntry(lookup: ReferenceLookup): ReferenceEntry | null {
  const { target } = lookup
  const page = lookup.pages.get(target.page)
  if (!page || target.y === null) return null
  const spans = usable(page)
  const family = destinationFamily(target.name)
  const siblings = family
    ? lookup.destinations.filter(d => d.page === target.page && destinationFamily(d.name) === family)
    : []
  const raise = pageRaise(page, spans, siblings)
  const first = startSpan(page, spans, target.x, target.y, raise)
  if (!first) return null
  const fs = first.fontSize
  // The column starts at the text, not at the anchor: left of the text there may be line numbers.
  const left = (labelBefore(spans, first) ?? first).x - 0.6 * fs
  const col = columnAt(page, spans, left, baselineOf(first), fs)
  const start = col.lines.findIndex(l => l.spans.includes(first))
  if (start < 0) return null
  const head = col.lines[start]

  // The column's leading: the distance between the lines of one entry. The lower quartile of the
  // gaps around the start, because the larger ones are the space between entries.
  // Only lines in the entry's own size count: body text above a bibliography is often larger.
  const gaps: number[] = []
  const ownSize = (l: Line) => Math.abs(l.fs - fs) <= 0.06 * fs
  for (let i = Math.max(1, start - 25); i < col.lines.length && i <= start + 25; i++) {
    const g = col.lines[i].baseline - col.lines[i - 1].baseline
    if (ownSize(col.lines[i]) && ownSize(col.lines[i - 1]) && g > 0.8 * fs && g < 3 * fs) gaps.push(g)
  }
  gaps.sort((a, b) => a - b)
  const leading = gaps.length ? gaps[Math.floor((gaps.length - 1) / 4)] : 1.2 * fs

  // The next destination in this column closes the entry. Only another line counts: aliases of
  // the target (biblatex writes two names per entry) start on the same line.
  // A destination belongs to the line of the span it points at (its own x may lie in the margin).
  const anchors = new Map<RefDestination, FurnitureSpan | null>()
  const anchorOf = (d: RefDestination) => {
    if (!anchors.has(d)) {
      const p = lookup.pages.get(d.page)
      anchors.set(d, p && d.y !== null ? startSpan(p, d.page === target.page ? spans : usable(p), d.x, d.y, raise) : null)
    }
    return anchors.get(d)!
  }
  let stopAt = col.lines.length
  for (const d of lookup.destinations) {
    if (d.page !== target.page) continue
    const span = anchorOf(d)
    if (!span) continue
    const i = col.lines.findIndex(l => l.spans.includes(span))
    if (i > start && i < stopAt) stopAt = i
  }
  const bounded = stopAt < col.lines.length

  // Does this column use hanging indents (continuation lines set in from the entry's first)?
  const flush = head.x0
  let indented = 0
  let flushCount = 0
  for (let i = start + 1; i < Math.min(col.lines.length, start + 40); i++) {
    const dx = col.lines[i].x0 - flush
    if (dx > 0.5 * fs && dx < 4 * fs) indented++
    else if (Math.abs(dx) <= 0.35 * fs) flushCount++
  }
  const hanging = indented >= 3 && indented >= 0.25 * (indented + flushCount)
  const headLabel = labelOf(lineText(head))

  const taken: Line[] = [head]
  let ranOff = true
  const maxLines = 80
  for (let i = start + 1; i < stopAt && taken.length < maxLines; i++) {
    const line = col.lines[i]
    if (bareNumber(line)) continue
    const gap = line.baseline - taken[taken.length - 1].baseline
    // A gap no entry has inside it: the end of the column's text (a footer, a figure, the end of
    // the list). The entry may still go on in the next column.
    if (gap > Math.max(2.2 * fs, 1.7 * leading) || !sameType(line, fs) || outOfColumn(col, line, flush, fs, head)) break
    const text = lineText(line)
    const atFlush = Math.abs(line.x0 - flush) <= 0.35 * fs
    // Signs of the next entry that hold even when a boundary destination lies further down (an
    // entry without an anchor): the next label at the entry's own x ("2." after "1.", while a
    // "[CrossRef]" on an indented line is not one), or a line back at the first line's x after
    // this entry's own second line was indented.
    const label = atFlush && headLabel ? labelOf(text) : null
    let next = !!label && (headLabel!.n === null || (label.n !== null && label.n !== headLabel!.n))
    if (atFlush && taken.length > 1 && taken[1].x0 - flush > 0.5 * fs) next = true
    // Without a boundary, the layout alone decides: the space between entries, or a hanging column.
    if (!bounded && (gap > leading + 0.25 * fs || (hanging && atFlush))) next = true
    if (next) { ranOff = false; break }
    taken.push(line)
  }
  if (bounded || taken.length >= maxLines) ranOff = false
  // A sibling anchor below the last line, with no line under it, belongs to an entry that starts
  // on the next page (hyperref leaves the anchor behind when the page breaks before the entry).
  // Anchors of one column share their x, wherever that lies relative to the text.
  const last = taken[taken.length - 1]
  if (ranOff && target.x !== null && siblings.some(d => d !== target && d.x !== null && d.y !== null
    && Math.abs(d.x - target.x!) <= 1.5 * fs && d.y > last.baseline - 0.25 * fs && !anchorOf(d))) ranOff = false

  // Ran off the bottom of the column: continue above the next sibling destination, if it lies in
  // a later column or on the next page.
  let tail: Line[] = []
  if (ranOff && family) {
    const here = readingKey(target.page, page, first)
    let next: { span: FurnitureSpan; page: FurniturePage } | null = null
    let nextKey: [number, number, number] | null = null
    for (const d of lookup.destinations) {
      if (d === target || destinationFamily(d.name) !== family) continue
      if (d.page !== target.page && d.page !== target.page + 1) continue
      const span = anchorOf(d)
      const p = lookup.pages.get(d.page)
      if (!span || !p || (d.page === target.page && centerX(span) < col.right)) continue
      const k = readingKey(d.page, p, span)
      if (!before(here, k)) continue
      if (!nextKey || before(k, nextKey)) { next = { span, page: p }; nextKey = k }
    }
    if (next) tail = continuationBefore(next.page, next.span, fs, leading)
  }

  const text = joinLines([...taken, ...tail].map(lineText).filter(Boolean), wordsOf(lookup.pages.values()))
  if (!text) return null
  return { text: text.length > MAX_TEXT ? text.slice(0, MAX_TEXT) + '…' : text, bounded }
}
