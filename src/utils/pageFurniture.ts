/**
 * Page furniture: the text that is PRINTED ON a page but is not part of the text flow
 * — page numbers, running headers / footers, footnotes, margin stamps, and the figures
 * and tables set across the flow (captions, cells, chart labels) — and the rule for
 * leaving it out of a highlight that crosses a page break.
 *
 * WHY. A drag from the last line of page N to the first line of page N+1 selects, in
 * DOM order, everything in between: the footnotes and the page number at the foot of
 * page N and the running header / page number at the head of page N+1. The highlight
 * then paints the page number yellow and stores "... each considered composition. 2In
 * contrast, our ..." as its text. This module finds those spans so the viewer can drop
 * them. It is pure geometry + text on data the viewer already has (the pdf.js text
 * layer): no PDF parsing, no canvas, no dependencies.
 *
 * ── API ────────────────────────────────────────────────────────────────────────
 *   planFurnitureDrop(pages, opts?)       THE ONE CALL for the viewer: the selected pages in, the
 *                                         span indices to leave out of each page out
 *   planFurnitureDropReport(pages, opts?) the same plus what was left out (kind + text of each span,
 *                                         counts by kind) for a notice / "include them" toggle
 *   spansFromTextLayer(layer, page, k)    DOM: a rendered pdf.js text layer -> SpanReading
 *   selectedTextBySpan(range, elements)   DOM: the selected text of every span (and so which spans
 *                                         the selection starts / ends in)
 *   keptSelectionText(sel, eol, dropped)  one page's selected text without the dropped spans
 *   keptSelectionTextAcrossPages(pages)   the whole highlight text without them (use this one)
 *   furniturePageFromTextContent(tc, view)  the same FurniturePage from page.getTextContent() — no
 *                                         rendering: neighbours and body size of pages not on screen
 *   preferContentGeometry(dom, content)   classify with pdf.js's own geometry instead of the browser's: the
 *                                         DOM reading's spans, with `content`'s x / y / w / h when both list
 *                                         the same texts in the same order (WebKit measures text wider)
 *   estimateBodyFontSize(pages)           the document's normal type size (CSS px at scale 1)
 *   classifyPageFurniture(page, opts?)    one entry per span: null (= text flow) or a kind
 *   classifyPageLayout(page, opts?)       the same, plus which float (figure / table) each 'float' span is part of
 *   captionHead(text)                     "Figure 3:", "Fig. 2.", "TABLE IV", "图 3" — whether text starts like a caption
 *   furnitureToDrop(selectedPages)        the cross-page POLICY on classified pages (what
 *                                         planFurnitureDrop applies)
 *   isPageNumberText(text)                "3", "iv", "Page 3 of 15", "- 3 -", "第3页"
 *
 * ── COORDINATES ────────────────────────────────────────────────────────────────
 * CSS px AT SCALE 1, origin = page top-left, y grows downward — i.e. the rect of a
 * text-layer <span> relative to its page element, divided by the viewer's `scale`.
 * `fontSize` is the PDF font size in the same unit (pdf.js's `--font-height`), not the
 * zoomed one; `angle` is in degrees (CSS rotate(): 0 = upright, -90 = reads bottom to
 * top), absent for horizontal text. One span = one pdf.js text item = one <span>.
 * A span's box is pdf.js's: it is `fontSize` tall and starts ~0.2 em above the glyphs'
 * cap line (baseline = y + ~0.8 h), exactly what the selection highlight is painted
 * from. Nothing here depends on that ratio beyond a few tenths of an em: the reading from
 * the DOM and the one from `getTextContent()` differ by ~0.4 px in y (the browser's ascent
 * of the span's font versus the 0.8 assumed) and classify identically.
 *
 * ── POLICY (what `furnitureToDrop` / `planFurnitureDrop` implement) ────────────
 * Only a selection that REALLY crosses a page boundary loses anything, and only the
 * sides that face the boundary:
 *   · TAIL furniture (footnote / footer / a page number at the bottom) of every
 *     selected page that continues onto the next page;
 *   · HEAD furniture (header / a page number at the top) of every selected page that
 *     continues from the previous page;
 *   · MARGIN stamps (rotated text in the page margin, e.g. the arXiv stamp) of every
 *     selected page of a cross-page selection;
 *   · FLOATS (a figure's or a table's caption and text) on every selected page of a cross-page
 *     selection, wherever they stand — except the float the user's own start / end lies in;
 *   · never when the selection stays inside one page;
 *   · furniture the text layer lists on the far side of the user's own end point is swept in by DOM
 *     order only: on the LAST page a tail span listed before the span the user ended in (the foot page
 *     number that comes first in an ACL / EMNLP page), on the FIRST page a head span listed after the
 *     span the user started in (a running head that comes last) goes as well;
 *   · never a side on which the user's own start / end point lies inside furniture:
 *     they pointed at it on purpose;
 *   · never when it would leave nothing at all (`planFurnitureDrop`).
 *
 * ── WHAT COUNTS (classifyPageFurniture) ────────────────────────────────────────
 * Text is grouped into SEGMENTS (a run on one baseline without a gap wider than 1 em; a
 * gap with an explicit space fragment in it, up to 2 em and not across a detected column
 * gutter, is a stretched word space) and ROWS (all segments on one baseline). Everything
 * is measured in units of the body size B (`estimateBodyFontSize`: the character-weighted
 * mode, over three adjacent 0.25 px buckets, of horizontal text).
 *   pageNumber  a row that is only a number ("7", "iv", "Page 3 of 15", "A-3") within 16 % of the
 *               page edge, isolated (>= 0.55 B from the text, or 0.05 B when it is centred on the
 *               page: ACL and Word set it a third of an em under the last line; >= 1.5 B in a
 *               typewriter face), not oversized, not one of a column of numbers; or a plain number
 *               set at the end of a header row ("2 | Qiu et al.", "title ... 14"). A weak form — a lone
 *               roman letter ("x", "I"), a letter glued to digits ("x2", "Q1", "S12") — counts only when a
 *               neighbour repeats a number in that place or it is centred in the outer 8.5 % strip.
 *   header/footer  a short label (<= 100 glyphs, <= 4 segments) in the outer 14 % (8.5 % "strip",
 *               5 % "hairline"), set apart from the text (>= 1.25 B; 0.9 B for a centred running title
 *               or a line that names itself; 0.5 B on the hairline), smaller than the body (0.7–0.95 B;
 *               up to 1.2 B when it names itself: "Published as a conference paper at ICLR", "©",
 *               "Proceedings", a URL, an author head "Qiu et al.") — and not prose, not a caption or a
 *               credit line ("Abb. 3: …", "図3：…", "(b) …", "Source: …"), not a heading of the paper's
 *               structure ("References", "5 RELATED WORK"). On a slide an author name or a URL needs
 *               a neighbour that repeats it, or the very edge of the page. A line another page repeats (same text
 *               modulo digits, same place; `neighbours`) needs only 0.4 B, may be larger and sit
 *               further in — but a caption, a "(continued)" label, a table heading, a slide title or
 *               a number that is the same on both pages is repeated CONTENT, not furniture.
 *   footnote    a block of notes at the foot of a page, in type 0.55–0.925 B, BELOW all body-size
 *               text of its column and under some of it, not in the middle of the flow; it begins
 *               (any of its lines) with a mark — a symbol (* † ‡ § ¶), a raised digit / letter that
 *               is smaller than the note, or a digit glued to it ("1Equal") — and a note that merely
 *               continues one from the previous page or follows an accepted block (a venue notice
 *               under the author notes) is taken with it. Unmarked small print only when it is a
 *               short notice (venue, licence, e-mail, URL). Not tables (aligned short cells, numbers),
 *               not listings (typewriter face), not reference lists ([1], a "References" heading),
 *               not captions, not centred lines, not display equations (lines above the first note
 *               that are not flush with the column are cut off).
 *   margin      rotated text in the outer 7 % (x) / 4 % (y): the arXiv stamp, side watermarks.
 *   float       a figure, table or algorithm: its CAPTION — a block that starts with a label, a number
 *               and a delimiter ("Figure 3:", "Fig. 2.", "Table 1", "TABLE IV" over its title, "Algorithm 1
 *               Training", "图 3：") and does not continue the paragraph above it (same size, closer than
 *               0.6 em) — and its TEXT, walked outwards from the caption inside the caption's column: a
 *               figure's labels above it (past the picture's white band only small type within its
 *               width), a table's rows on the side that holds rows of cells, an algorithm's numbered
 *               steps; every walk stops at the first line of running text, a heading, a numbered
 *               display, another caption or the page furniture. A caption in the size of the text must
 *               show its float (labels, rows, or a picture's band of >= 2.5 em), or be at most four lines
 *               set off from what follows: "Figure 1. At the top level, …" opening a column is a paragraph.
 *
 * ── DESIGN RULE: BE CONSERVATIVE ───────────────────────────────────────────────
 * A false positive silently drops text the user wanted and cannot be recovered; a
 * false negative merely leaves today's behaviour. Every rule below therefore asks for
 * several independent signals (position, size relative to the body, a gap, the shape
 * of the text) and, when in doubt, answers null.
 *
 * ── FOR THE INTEGRATOR ─────────────────────────────────────────────────────────
 * At mouse-up, for a selection `range` that touches two or more pages:
 *   1. For each touched page, in page order: `rd = spansFromTextLayer(textLayerEl, pageEl, scale)`
 *      and `selected = selectedTextBySpan(range, rd.elements)`. (Every touched page needs a rendered
 *      text layer anyway: the highlight rects come from it.)
 *   2. `drop = planFurnitureDrop(pages.map(p => ({ page: p.rd.page, selected: p.selected })), { bodyFontSize,
 *      neighbours })`. Pass `bodyFontSize` from the whole DOCUMENT when you can (e.g. `estimateBodyFontSize`
 *      of `furniturePageFromTextContent(await page.getTextContent(), page.view, page.rotate)` of the first
 *      ~8 pages, cached per document): from two pages alone it is wrong on ~5 % of pages (a page of
 *      tables / figures), always towards flagging less. Pass as `neighbours` any other pages of the document
 *      you have at hand — the pages next to the selection, read with `furniturePageFromTextContent` — and
 *      NEVER the selected page itself: a repeated header / page number on neighbours is the strongest evidence
 *      there is (running heads 87 % -> 95 % found), and the other selected pages are already used.
 *   3. Per page `i`: in the walk over the selection's text nodes that builds the rects, skip every text
 *      node whose span element index (`rd.elements.indexOf(span)`, or a Map built once) is in `drop[i]`.
 *      Rebuild the text with `keptSelectionTextAcrossPages(pages.map((p, i) => ({ selected: p.selected,
 *      eolAfter: p.rd.eolAfter, dropped: drop[i] })))` — it is `sel.toString().trim()` minus the dropped
 *      spans, with a single "\n" left where a dropped page number stood between two lines (and one at
 *      every page boundary, which the browser's own string may lack — see `keptSelectionTextAcrossPages`).
 *   4. A page left without any rect / text simply gets no highlight record. If NO page keeps anything
 *      `planFurnitureDrop` has already returned empty sets: keep the selection as it was.
 *   5. Fail safe: rebuild the text once with nothing dropped (`dropped` omitted) and compare it with
 *      `sel.toString()` IGNORING WHITESPACE; if they differ the text layer is not the plain pdf.js one this
 *      module was verified against (nested spans, injected nodes) — do not trim anything. Comparing line
 *      breaks too makes the check fail on every cross-page selection in an app whose shell is
 *      `user-select: none` (see `keptSelectionTextAcrossPages`). For a notice ("skipped a page
 *      number and 2 footnote lines") and an "include them" toggle use `planFurnitureDropReport` (its `counts`
 *      and `items`) and keep both variants — the original rects / text and the trimmed ones.
 * Cost: `spansFromTextLayer` ~0.3–1.6 ms per page (one layout pass), `classifyPageFurniture` 0.1 ms median /
 * 0.75 ms p99 per page (about 4x that with 4 neighbours), `estimateBodyFontSize` 0.2 ms for two dense pages.
 * Nothing here runs per frame.
 */

// ═══════════════════════════════════════════════════════════════════════════════
// Types
// ═══════════════════════════════════════════════════════════════════════════════

export interface FurnitureSpan {
  text: string
  x: number
  y: number
  w: number
  h: number
  fontSize: number
  /** Degrees; absent / 0 = horizontal. */
  angle?: number
  /** Set in a monospace face (pdf.js maps the PDF font's family to a CSS generic one, and
   *  `spansFromTextLayer` reads that). Code and prompt listings are the one kind of small
   *  type that must never be taken for a footnote or a running head. Optional: leave it
   *  out and nothing is assumed. */
  mono?: boolean
}

export interface FurniturePage {
  width: number
  height: number
  spans: FurnitureSpan[]
}

/** What `spansFromTextLayer` reads off a rendered page. */
export interface SpanReading {
  page: FurniturePage
  /** `elements[i]` is the text-layer <span> of `page.spans[i]`, in DOM order. */
  elements: HTMLElement[]
  /** Whether a line break (pdf.js's <br>) follows `elements[i]`. */
  eolAfter: boolean[]
}

export type FurnitureKind = 'pageNumber' | 'header' | 'footer' | 'footnote' | 'margin' | 'float'

export interface ClassifyOptions {
  /** `estimateBodyFontSize` of the document's pages. Without it the page's own mode is used. */
  bodyFontSize?: number
  /** Other pages of the same document (ideally the one before and the one after, or two
   *  of each): a line that repeats at the same place on them is a running header / footer
   *  / page number, which lets the rules be much looser. Cost: one extra row-building
   *  pass per neighbour. */
  neighbours?: readonly FurniturePage[]
}

// ═══════════════════════════════════════════════════════════════════════════════
// Small helpers
// ═══════════════════════════════════════════════════════════════════════════════

/** Rotation below this many degrees is still "horizontal" (skewed scans). */
const HORIZONTAL_DEG = 3

function isHorizontal(s: FurnitureSpan): boolean {
  const a = s.angle
  return a === undefined || a === 0 || Math.abs(a) <= HORIZONTAL_DEG
}

function isBlank(text: string): boolean {
  return text.trim() === ''
}

/** Non-whitespace characters: the weight of a span in every statistic. */
function glyphCount(text: string): number {
  let n = 0
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i)
    if (c > 0x20 && c !== 0xa0 && c !== 0x3000) n++
  }
  return n
}

function squash(text: string): string {
  return text.replace(/\s+/g, ' ').trim()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Body font size
// ═══════════════════════════════════════════════════════════════════════════════

/** Font sizes are bucketed to this many px before the mode is taken. */
const FS_BUCKET = 0.25
/** Fewer glyphs than this over all the pages given = "too little text to tell". */
const MIN_BODY_GLYPHS = 150

/**
 * The size most of the text is set in: the character-weighted mode of the font sizes
 * of horizontal text. The mode is taken over a sliding window of three buckets so a
 * body that is 9.96 px on one line and 10.0 on the next still counts as one size, and
 * the answer is the weighted mean inside the winning window.
 *
 * Rotated text (the arXiv stamp, side watermarks, y-axis labels) is left out, so it
 * cannot distort the statistic. Returns null when the pages hold too little text.
 */
export function estimateBodyFontSize(pages: readonly FurniturePage[]): number | null {
  const bins = new Map<number, { glyphs: number; sum: number }>()
  let total = 0
  for (const p of pages) {
    for (const s of p.spans) {
      if (!isHorizontal(s) || !(s.fontSize > 0)) continue
      const n = glyphCount(s.text)
      if (n === 0) continue
      const k = Math.round(s.fontSize / FS_BUCKET)
      const b = bins.get(k)
      if (b) { b.glyphs += n; b.sum += n * s.fontSize } else bins.set(k, { glyphs: n, sum: n * s.fontSize })
      total += n
    }
  }
  if (total < MIN_BODY_GLYPHS) return null

  let bestKey = 0
  let bestScore = -1
  let bestOwn = -1
  for (const [k, b] of bins) {
    const score = b.glyphs + (bins.get(k - 1)?.glyphs ?? 0) + (bins.get(k + 1)?.glyphs ?? 0)
    if (score > bestScore || (score === bestScore && b.glyphs > bestOwn)) {
      bestKey = k; bestScore = score; bestOwn = b.glyphs
    }
  }
  let glyphs = 0
  let sum = 0
  for (const k of [bestKey - 1, bestKey, bestKey + 1]) {
    const b = bins.get(k)
    if (b) { glyphs += b.glyphs; sum += b.sum }
  }
  return glyphs > 0 ? sum / glyphs : null
}

// ═══════════════════════════════════════════════════════════════════════════════
// Rows and segments
// ═══════════════════════════════════════════════════════════════════════════════
//
// pdf.js hands out text in fragments (a word, a run in one font, a whole line — it
// depends on the producer). Everything below works on SEGMENTS: runs of fragments that
// sit on one baseline with no wide gap between them. A ROW is every segment on one
// baseline. Segments, not rows, are the unit wherever columns matter: in a two-column
// page the left and the right column share baselines, and a footnote under the left
// column must not be welded to the right column's body line next to it.

/** Two fragments share a row when their vertical centres differ by less than this many
 *  times the larger fragment's height. 0.4 < half a normal leading, so neighbouring
 *  lines never merge, while a superscript or subscript (centre ~0.25 em off) still does. */
const ROW_TOL = 0.4
/** A horizontal gap wider than this many times the larger fragment's height ends a
 *  segment: a column gutter, a tab, a right-aligned page number. Word spaces stay below it. */
const SEG_GAP = 1.0
/** ...except that a gap with an explicit space fragment starting in it, this wide at most, is a
 *  stretched word space (a short justified line of a footnote has been measured at 1.4 em) — unless it
 *  lies across a column gutter or ends at a bare page number. Wider gaps are left alone: tables and tab
 *  stops look the same, and splitting a line is the harmless mistake. */
const BRIDGE_GAP = 2.0

interface Seg {
  /** Index in `Layout.segs`. */
  id: number
  /** Every span of the segment (the blank ones inside it included), in x order. */
  spans: number[]
  text: string
  x0: number
  x1: number
  y0: number
  y1: number
  cy: number
  /** Character-weighted font size. */
  fs: number
  glyphs: number
  /** Share of the glyphs set in a monospace face. */
  mono: number
  row: number
}

interface Row {
  segs: Seg[]
  /** Union of the segments' boxes. */
  top: number
  bottom: number
  cy: number
  fs: number
  glyphs: number
  /** Share of the glyphs set in a monospace face. */
  mono: number
  text: string
}

interface Layout {
  rows: Row[]
  segs: Seg[]
  /** Segment index of every span, -1 for rotated ones and for blanks that fit nowhere. */
  segOf: Int32Array
}

/** Rows with at least this many glyphs vote for column gutters. */
const GUTTER_MIN_GLYPHS = 15

/**
 * Horizontal bands that are empty on most text rows of the page: the gutters between columns.
 * A gap with an explicit space fragment in it is normally a stretched word space — but some
 * PDFs end every line with a space fragment, and then the gutter looks exactly like one. A
 * real gutter is the same place on row after row (a stretched space is not), so the bands
 * found here are never bridged.
 */
function findGutters(spans: FurnitureSpan[], rows: number[][]): Array<[number, number]> {
  const gaps: Array<[number, number]> = []
  let textRows = 0
  let hSum = 0
  let hN = 0
  for (const members of rows) {
    const solid = members.filter(i => !isBlank(spans[i].text)).sort((a, b) => spans[a].x - spans[b].x)
    let glyphs = 0
    for (const i of solid) { glyphs += glyphCount(spans[i].text); hSum += spans[i].h; hN++ }
    if (glyphs < GUTTER_MIN_GLYPHS) continue
    textRows++
    let right = -Infinity
    let lastH = 0
    for (const i of solid) {
      const s = spans[i]
      if (right > -Infinity && s.x - right > SEG_GAP * Math.max(s.h, lastH) && s.x - right <= 4 * Math.max(s.h, lastH)) gaps.push([right, s.x])
      right = Math.max(right, s.x + s.w)
      lastH = s.h
    }
  }
  if (textRows < 6 || hN === 0) return []
  const bin = Math.max(1, 0.5 * hSum / hN)
  const counts = new Map<number, number>()
  for (const [lo, hi] of gaps) for (let b = Math.floor(lo / bin); b <= Math.floor(hi / bin); b++) counts.set(b, (counts.get(b) ?? 0) + 1)
  const need = Math.max(6, 0.2 * textRows)
  const hot = [...counts].filter(([, c]) => c >= need).map(([b]) => b).sort((a, b) => a - b)
  const out: Array<[number, number]> = []
  for (const b of hot) {
    const last = out[out.length - 1]
    if (last && b * bin <= last[1] + 1e-6) last[1] = (b + 1) * bin
    else out.push([b * bin, (b + 1) * bin])
  }
  return out
}

function buildLayout(page: FurniturePage): Layout {
  const spans = page.spans
  const solid: number[] = []
  const blank: number[] = []
  for (let i = 0; i < spans.length; i++) {
    const s = spans[i]
    if (!isHorizontal(s)) continue
    if (isBlank(s.text)) blank.push(i)
    else solid.push(i)
  }
  const cyOf = (i: number) => spans[i].y + spans[i].h / 2
  solid.sort((a, b) => cyOf(a) - cyOf(b))

  // 1. Rows: sweep by vertical centre, join the closest open row within tolerance.
  interface Acc { cy: number; wt: number; h: number; members: number[] }
  const accs: Acc[] = []
  for (const i of solid) {
    const s = spans[i]
    const cy = cyOf(i)
    const wt = Math.max(1, glyphCount(s.text))
    let best: Acc | null = null
    let bestD = Infinity
    for (let r = accs.length - 1; r >= 0; r--) {
      const a = accs[r]
      const reach = ROW_TOL * Math.max(s.h, a.h)
      if (cy - a.cy > 2 * reach + 1) break
      const d = Math.abs(cy - a.cy)
      if (d <= reach && d < bestD) { best = a; bestD = d }
    }
    if (best) {
      best.cy = (best.cy * best.wt + cy * wt) / (best.wt + wt)
      best.wt += wt
      if (s.h > best.h) best.h = s.h
      best.members.push(i)
    } else {
      accs.push({ cy, wt, h: s.h, members: [i] })
    }
  }
  accs.sort((a, b) => a.cy - b.cy)

  // Blank fragments (explicit spaces) bridge gaps inside a line, so each goes to the
  // row it sits on. A blank that sits on no row is simply left out.
  for (const i of blank) {
    const cy = cyOf(i)
    let lo = 0
    let hi = accs.length - 1
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (accs[mid].cy < cy) lo = mid + 1
      else hi = mid
    }
    let best: Acc | null = null
    let bestD = Infinity
    for (const r of [lo - 1, lo]) {
      const a = accs[r]
      if (!a) continue
      const d = Math.abs(cy - a.cy)
      if (d <= ROW_TOL * Math.max(spans[i].h, a.h) && d < bestD) { best = a; bestD = d }
    }
    if (best) best.members.push(i)
  }

  const gutters = findGutters(spans, accs.map(a => a.members))

  // 2. Segments: split every row at wide gaps. The gaps are measured between SOLID fragments
  //    only, and a blank (explicit space) fragment bridges one by where it STARTS, never by how
  //    wide it is: that width is the PDF's own in one reader and the glyph's natural width in the
  //    text layer of a browser, and the two readings must classify alike. A blank joins the
  //    segment it sits in (or none).
  const segs: Seg[] = []
  const rows: Row[] = []
  const segOf = new Int32Array(spans.length).fill(-1)
  for (const acc of accs) {
    const m = acc.members.sort((a, b) => spans[a].x - spans[b].x || a - b)
    const rowSegs: Seg[] = []
    const runs: number[][] = []
    const blanks = m.filter(i => isBlank(spans[i].text))
    const lastSolid = m.filter(i => !isBlank(spans[i].text)).pop()
    let run: number[] = []
    let right = -Infinity
    let lastH = 0
    for (const i of m) {
      const s = spans[i]
      if (isBlank(s.text)) continue
      const reach = Math.max(s.h, lastH)
      if (run.length > 0 && s.x - right > SEG_GAP * reach) {
        // A wide gap ends the segment — unless an explicit space fragment starts inside it and
        // the gap is no wider than a badly stretched word space: a justified line, not a gutter.
        // A bare number at either end of the row is never bridged to the text (the page
        // number of a running head is set off by a tab or a stretched space just like that).
        const numberEnd = i === lastSolid ? isBareNumber(s.text) : run.length === 1 && isBareNumber(spans[run[0]].text)
        const bridged = !numberEnd && s.x - right <= BRIDGE_GAP * reach &&
          blanks.some(b => spans[b].x >= right - 0.5 * reach && spans[b].x <= s.x) &&
          !gutters.some(([lo, hi]) => lo < s.x && hi > right)
        if (!bridged) { runs.push(run); run = []; right = -Infinity }
      }
      run.push(i)
      right = Math.max(right, s.x + s.w)
      lastH = s.h
    }
    if (run.length > 0) runs.push(run)
    // A blank joins the run whose extent holds its left edge (a space between two words of
    // one line); one that sits in a real gap joins nothing.
    const extent = runs.map(r => ({ x0: spans[r[0]].x, x1: r.reduce((mx, k) => Math.max(mx, spans[k].x + spans[k].w), -Infinity) }))
    for (const i of blanks) {
      const bx = spans[i].x
      const ri = extent.findIndex(e => bx >= e.x0 - 0.1 * spans[i].h && bx <= e.x1 + 0.1 * spans[i].h)
      if (ri >= 0) runs[ri].push(i)
    }
    for (const r of runs) {
      r.sort((a, b) => spans[a].x - spans[b].x || a - b)
      const seg = makeSeg(spans, r)
      if (seg) { seg.id = segs.length; segs.push(seg); rowSegs.push(seg); for (const k of seg.spans) segOf[k] = seg.id }
    }
    if (rowSegs.length === 0) continue
    let top = Infinity, bottom = -Infinity, wt = 0, cy = 0, fsSum = 0, glyphs = 0, monoSum = 0
    for (const sg of rowSegs) {
      top = Math.min(top, sg.y0); bottom = Math.max(bottom, sg.y1)
      wt += sg.glyphs; cy += sg.cy * sg.glyphs; fsSum += sg.fs * sg.glyphs; glyphs += sg.glyphs; monoSum += sg.mono * sg.glyphs
    }
    rows.push({
      segs: rowSegs, top, bottom,
      cy: wt > 0 ? cy / wt : (top + bottom) / 2,
      fs: wt > 0 ? fsSum / wt : 0,
      glyphs,
      mono: glyphs > 0 ? monoSum / glyphs : 0,
      text: rowSegs.map(sg => sg.text).join('   '),
    })
  }
  rows.sort((a, b) => a.cy - b.cy)
  rows.forEach((r, ri) => r.segs.forEach(sg => { sg.row = ri }))
  return { rows, segs, segOf }
}

function makeSeg(spans: FurnitureSpan[], idx: number[]): Seg | null {
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity
  let glyphs = 0, fsSum = 0, cySum = 0, mono = 0
  let text = ''
  let prevRight = -Infinity
  for (const i of idx) {
    const s = spans[i]
    if (isBlank(s.text)) { if (text && !/\s$/.test(text)) text += ' '; continue }
    const n = glyphCount(s.text)
    x0 = Math.min(x0, s.x); x1 = Math.max(x1, s.x + s.w)
    y0 = Math.min(y0, s.y); y1 = Math.max(y1, s.y + s.h)
    glyphs += n; fsSum += n * s.fontSize; cySum += n * (s.y + s.h / 2)
    if (s.mono) mono += n
    if (text && !/\s$/.test(text) && !/^\s/.test(s.text) && s.x - prevRight > 0.12 * s.fontSize) text += ' '
    text += s.text
    prevRight = s.x + s.w
  }
  if (glyphs === 0) return null
  return { id: -1, spans: idx, text: squash(text), x0, x1, y0, y1, cy: cySum / glyphs, fs: fsSum / glyphs, glyphs, mono: mono / glyphs, row: -1 }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Text shapes
// ═══════════════════════════════════════════════════════════════════════════════

const ROMAN = /^(?=[ivxlcdm]+$)m{0,3}(?:cm|cd|d?c{0,3})(?:xc|xl|l?x{0,3})(?:ix|iv|v?i{0,3})$/i
const PAGE_WORD = '(?:page|pág\\.?|página|pagina|seite|p\\.?|pp\\.?|s\\.)'
const OF_WORD = '(?:of|/|von|de|sur|\\|)'
const PAGE_NUMBER_PATTERNS: RegExp[] = [
  /^\d{1,5}$/,
  // "- 3 -", "— 3 —", "· 3 ·", "– 3 –"
  /^[-–—‒―·•|\s]+\d{1,5}[-–—‒―·•|\s]+$/,
  // "Page 3", "Page 3 of 15", "p. 3", "3 of 15", "3 / 15", "3/15"
  new RegExp(`^${PAGE_WORD}\\s*\\d{1,4}(?:\\s*${OF_WORD}\\s*\\d{1,4})?$`, 'i'),
  new RegExp(`^\\d{1,4}\\s*${OF_WORD}\\s*\\d{1,4}$`, 'i'),
  // CJK: 第3页, 第 3 页 共 15 页, 3 页, 共15页
  /^第\s*\d{1,4}\s*页(?:\s*[,，/／]?\s*共\s*\d{1,4}\s*页)?$/,
  /^\d{1,4}\s*页$/,
  /^共\s*\d{1,4}\s*页$/,
]
/** "A-3", "S12", "E.4", "3-14": page numbers of appendices and of some book series — but just as
 *  well the number of a section or a range, so they count only when they stand alone on a line. */
const SECTION_STYLE_NUMBERS: RegExp[] = [
  /^[A-Za-z]{1,2}[-–.]?\d{1,3}$/,
  /^\d{1,3}[-–]\d{1,3}$/,
]

/** A bare integer (1–4 digits) or roman numeral: the page number of a running head, as opposed to the
 *  section-style "E.4" or "A-3" that only means something alone on its line. */
function isBareNumber(text: string): boolean {
  const t = text.trim()
  if (t.length === 0 || t.length > 8) return false
  if (/^\d{1,4}$/.test(t)) return true
  return ROMAN.test(t) && (t === t.toLowerCase() || t === t.toUpperCase())
}

/** A roman numeral that is more often something else: one letter ("I", "x", "C": a panel or axis label, a
 *  variable), one with an M or D (a thousand, five hundred: "mm", "mix", "di" — not a page of front matter), or a
 *  two-letter one starting with L / C ("li", "ci"). "ii", "iv", "xv", "xl" and the like are page numbers. */
function isWeakRoman(t: string): boolean {
  return t.length === 1 || /[dm]/i.test(t) || (t.length === 2 && /^[lc]/i.test(t))
}

/** Text that reads as a page number only by the way it is written, not by being a plain integer: a lone roman
 *  letter ("x", "I"), a rare roman word ("mi"), a letter glued to digits ("x2", "Q1", "S12"). Anywhere in a page
 *  they are variables, panel labels and question numbers, so the classifier takes one for a page number only when
 *  the page it is on says so (a neighbour page repeats a number in that very place, or it is centred in the outer
 *  strip, where page numbers are set). Integers, "- 3 -", "Page N" and "A-3" / "E.4" are not weak. */
export function isWeakPageNumberText(text: string): boolean {
  const t = squash(text)
  if (ROMAN.test(t) && (t === t.toLowerCase() || t === t.toUpperCase())) return isWeakRoman(t)
  return /^[A-Za-z]{1,2}\d{1,3}$/.test(t)
}

/** Whether the whole text is nothing but a page number ("3", "iv", "Page 3 of 15", "- 3 -").
 *  `strict` is for text standing next to other text: it leaves out the section-style forms ("A-3", "E.4"),
 *  which only a line of their own vouches for, and the weak roman letters ("C", "I": appendix and section labels). */
export function isPageNumberText(text: string, strict = false): boolean {
  const t = squash(text)
  if (t.length === 0 || t.length > 20) return false
  if (ROMAN.test(t)) return (t === t.toLowerCase() || t === t.toUpperCase()) && !(strict && isWeakRoman(t))
  for (const re of PAGE_NUMBER_PATTERNS) if (re.test(t)) return true
  if (!strict) for (const re of SECTION_STYLE_NUMBERS) if (re.test(t)) return true
  return false
}

/** Starts like a caption: "Figure 3", "Fig. 2.", "Table 1:", "Algorithm 2", "图 3", "表1" — and the same word in the
 *  languages a paper is written in: 図3 (ja), 그림 3 (ko), Abb. 3 / Abbildung 3 / Tabelle 2 (de), Figura 3 / Tabla 2 /
 *  Cuadro 1 / Gráfico 2 (es, it, pt), Figure 3 / Tableau 2 / Graphique 1 (fr), Рис. 3 / Таблица 2 (ru), and the
 *  prefixes journals put in front: "Supplementary Fig. 3", "Extended Data Fig. 3 |", "Source Data Table 1".
 *  (No `\b`: JavaScript's word boundary knows ASCII letters only, so after "Рис" there is none.) */
const CAPTION_START = new RegExp(
  '^(?:(?:supplementary|extended\\s+data|source\\s+data|supporting)\\s+)?' +
  '(?:(?:fig(?:ure|ura|ur)?s?|table|tab|algorithm|alg|listing|scheme|chart|exhibit|panel|plate|' +
  'abb(?:ildung)?|tabelle|tableau|tabla|tabella|cuadro|gr[aá]fico|graphique|sch[eé]ma|' +
  'рис(?:унок)?|таблица|табл|схема)(?![\\p{L}])\\.?\\s*[\\p{L}\\p{N}]' +
  '|(?:附|补充)?[图表圖図]\\s*(?:[S\\d０-９]|[A-Z]\\d)|(?:그림|표)\\s*\\d)',
  'iu',
)

/** A sub-panel label that starts a line — "(b) Results on the test set", "b) …" — is the caption of a panel, and
 *  "Note:" / "Source:" / "Photo:" / 注：/ 资料来源： lead the credit line under a figure or table. Either one sits
 *  at the foot of a page as content, not as running text. ("(c) 2021 Elsevier" is a copyright line, not a panel.) */
const PANEL_OR_CREDIT_START = new RegExp(
  '^(?:(?:\\(\\s*[a-h]\\s*\\)|[a-h]\\))\\s*(?!(?:19|20)\\d\\d\\b|copyright|©)\\S' +
  '|(?:notes?|source|sources|data source|photos?|photo credit|image|image credit|credit|credits|courtesy)\\s*:' +
  '|(?:注|註|备注|備註|说明|說明|来源|來源|资料来源|資料來源|数据来源|數據來源|图片来源|圖片來源)\\s*[：:])',
  'iu',
)

/** The label of a line that is part of a figure / table / page content (a caption, a panel label, a credit line). */
function isContentLabel(label: string): boolean {
  const t = label.trim()
  return CAPTION_START.test(t) || PANEL_OR_CREDIT_START.test(t)
}

/** A heading of the paper's own structure: never a running head, whatever its size. */
const SECTION_HEADING =
  /^(?:(?:\d+(?:\.\d+)*|[A-Z])\.?\s+)?(?:abstract|introduction|conclusions?|references?|bibliography|acknowledg(?:e)?ments?|appendix(?:es)?|related work|background|discussion|results|methods?|summary|contents|table of contents|index|摘要|引言|结论|参考文献|目录|致谢)\s*$/i

/** "(continued)", "Table 2 (continued from previous page)": the repeated label of something that runs on. */
const CONTINUED = /\(?\s*(?:continued|cont\.?)\b|continued (?:from|on) (?:the )?(?:previous|next) page/i

/** A heading after which small type is a list of references / notes, not a footnote. */
const NOTES_HEADING =
  /^(?:(?:\d+(?:\.\d+)*|[A-Z])\.?\s+)?(?:references?|bibliography|references and notes|literature cited|works cited|cited literature|notes|endnotes|acknowledge?ments?|参考文献|参考资料|参考|引用文献|注释|附注)\s*:?$/i

/** A footnote symbol that stands alone as a fragment: needs no smaller size to be a mark. */
const SYMBOL_MARK = /^[*∗†‡§¶‖⋆★✝✠※]+$/u
/** A footnote mark that stands alone as a fragment. */
const FRAGMENT_MARK = /^(?:\d{1,2}|[*∗†‡§¶‖⋆★✝✠※]+|[¹²³⁴⁵⁶⁷⁸⁹⁰]+)$/u
/** First characters of a footnote that carry their own mark: a symbol, a superscript character, or a
 *  digit glued to the first word of the note ("1Equal", "2https://"). A plain digit is NOT enough
 *  ("3D reconstruction", "1 Baseline" are not notes) — see `startsWithFootnoteMark`, which also takes a
 *  digit that was set as a raised, smaller fragment. */
const FOOTNOTE_MARK = /^(?:\*+\s?(?=\p{Lu})|\*+(?=\p{L})|[∗†‡§¶‖⋆★✝✠※]+\s?(?=[\p{L}\p{N}(“"'\[])|\d{1,2}(?=\p{Lu}\p{Ll}|https?:|www\.|[AI]\s+\p{Ll})|[¹²³⁴⁵⁶⁷⁸⁹⁰]+\s*(?=[\p{L}(“"'\[]))/u
/** A raised mark is this much smaller than the note it belongs to. */
const SUPERSCRIPT_RATIO = 0.9

/** What an unmarked note at the foot of a page talks about: the venue, the licence, the
 *  authors' affiliation and contact, a URL. Small print that merely continues a column does not. */
const NOTICE_WORDS = new RegExp(
  '\\b(?:conference|proceedings|workshop|symposium|journal|transactions|preprint|under review|arxiv|copyright|' +
  'licen[sc]e[ds]?|creative commons|permission to|all rights reserved|published|accepted|received|manuscript|' +
  'submitted|correspondence|corresponding author|e-?mail|equal contribution|funding|supported by|are with|is with|' +
  'acm reference|doi|isbn)\\b|©|@|https?://|www\\.',
  'i',
)

/** What a running head / foot of a journal or a preprint says. Without a neighbour page to
 *  vouch for it, a line between the outer 5 % and 14 % of the page needs one of these (or a
 *  page number beside it) to be told from the first / last line of the text. */
const RUNNING_WORDS = new RegExp(
  '\\b(?:preprint|under review|conference|proceedings|workshop|symposium|journal|transactions|letters|' +
  'review article|research article|article|technical report|vol|published|accepted|submitted|manuscript|' +
  'doi|issn|isbn|copyright)\\b|©',
  'i',
)

/** "Qiu et al." / "Qiu et al. | Title": the author running head of a paper. "Smith et al., 2019", "Smith et al. (2019)",
 *  "et al. [12]" are citations — the credit line under a figure, a bullet of a slide — and say nothing about the page. */
function isAuthorRunningHead(label: string): boolean {
  const m = /\bet al\b\.?/i.exec(label)
  if (!m) return false
  return !/^\s*(?:[,;(\[]|\d)/.test(label.slice(m.index + m[0].length))
}

/** A label that is a web address with at most a couple of words beside it ("Article | https://doi.org/…",
 *  "www.nature.com/scientificreports"): the journal's own furniture. A sentence that merely contains a
 *  link — an entry of a list of papers — is not. */
function isUrlLabel(label: string): boolean {
  const tokens = label.split(/\s+/).filter(Boolean)
  let urls = 0
  let words = 0
  for (const t of tokens) {
    if (/^(?:https?:\/\/|www\.)\S+/i.test(t) || /\bdoi\.org\//i.test(t)) urls++
    else if (/\p{L}{3}/u.test(t)) words++
  }
  return urls > 0 && words <= 2
}

const NUMERIC_ONLY = /^[\d.,%+\-−–]+$/

const CJK = /[⺀-鿿豈-﫿＀-￯]/

const FUNCTION_WORD = /^(?:the|a|an|of|in|on|is|are|to|and|for|with|that|this|we|by|as|at|from|it|be|can|which|has|have|was|were|not|or|our|their|its)$/i

/** Text that reads as a sentence of the body flow rather than as a header / footer label. */
function looksLikeProse(text: string): boolean {
  const t = squash(text)
  if (CJK.test(t)) return glyphCount(t) > 40 || (/[。！？；]$/.test(t) && glyphCount(t) >= 12)
  // A line in capitals ("IEEE TRANSACTIONS ON ... VOL. 46, NO. 5") is a label, whatever its length.
  if (!/\p{Ll}/u.test(t)) return false
  // Separators ("|", "·", "–") are not words.
  const wordList = t.split(' ').filter(w => /[\p{L}\p{N}]/u.test(w))
  const words = wordList.length
  // A title (or an author list) capitalises most of its words; a sentence capitalises its first.
  const long = wordList.filter(w => /^\p{L}{4,}/u.test(w.replace(/^[(“"'\[]+/, '')))
  if (long.length >= 4 && long.filter(w => /^\p{Lu}/u.test(w.replace(/^[(“"'\[]+/, ''))).length >= 0.6 * long.length && !/[.]$/.test(t)) return false
  if (/^[a-z]/.test(t) && words >= 6) return true // starts mid-sentence ("npj Computational Materials" does not)
  if (words >= 14) return true
  if (words >= 6 && /[.!?;:,]$/.test(t) && !/\bet al\.$/i.test(t)) return true
  // Running text: several function words and punctuation inside ("Preprint. Under review." has neither).
  if (words >= 8 && /[.,;:]/.test(t) && wordList.filter(w => FUNCTION_WORD.test(w)).length >= 4) return true
  return false
}

/** Digits -> '#', lower case, one space: what "the same text on another page" means. */
function normalizeRepeat(text: string): string {
  return squash(text).toLowerCase().replace(/\d+/g, '#')
}

/** Share of the whitespace-separated tokens that are numbers ("0.82", "12,5%", "(3)"): a table of results. */
function numericTokenRatio(text: string): number {
  const tokens = text.split(/\s+/).filter(Boolean)
  if (tokens.length === 0) return 0
  let n = 0
  for (const t of tokens) if (/\d/.test(t) && /^[(\[]?[\d.,%+\-−–±×]+[)\]%]?$/.test(t)) n++
  return n / tokens.length
}

function digitRatio(text: string): number {
  let d = 0
  let n = 0
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i)
    if (c <= 0x20) continue
    n++
    if ((c >= 0x30 && c <= 0x39) || c === 0x25 || c === 0xb1) d++
  }
  return n === 0 ? 0 : d / n
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tunables (all lengths in units of the body font size B, "em")
// ═══════════════════════════════════════════════════════════════════════════════

/** Page numbers / headers / footers can only sit in this outer fraction of the page height. */
const EDGE_ZONE = 0.14
/** What a neighbour page vouches for may sit a little further in (a manuscript-format foot). */
const EDGE_ZONE_CONFIRMED = 0.2
/** A line that is only a number: this close to the edge at most (a page number that sits
 *  further in is part of the text block — a table, a listing, a chart). */
const EDGE_PAGE_NUMBER = 0.16
/** Free of any other evidence a header/footer TEXT line must lie in this tighter outer strip. */
const EDGE_STRIP = 0.085
/** Closer to the edge than any printed page keeps its text (a 0.5 in margin is 0.045 of an
 *  11 in page): a line here is margin furniture even if only a small gap separates it. */
const EDGE_HAIRLINE = 0.05
/** Smallest white gap (em) between a lone page number and the body: ACL on A4 puts it only
 *  ~1 em under the last line; running text never has an isolated numeric line. */
const GAP_PAGE_NUMBER = 0.55
/** Smallest gap (em) between a header / footer text line and the body. Body leading is
 *  0.1–0.3 em, a paragraph break ≤ 1 em (web pages: leading 0.6 em); running heads sit 1.1 em or more
 *  away (ICML 1.16, ACM 1.7, journals 2+). */
const GAP_RUNNING = 1.25
/** A running title (centred, smaller than the body) or a line that names itself sits a little closer. */
const GAP_RUNNING_TITLE = 0.9
/** The same outside the outer strip, where the last / first line of a full page of text lies. */
const GAP_RUNNING_WIDE = 1.8
const GAP_RUNNING_HAIRLINE = 0.5
/** ...and a line that names itself (a venue notice, a copyright line) needs hardly any. */
const GAP_RUNNING_HAIRLINE_NAMED = 0.15
/** A running head that no neighbouring page repeats is set below the body size (× body). */
const UNCONFIRMED_MAX_RATIO = 0.95
/** A centred running title is at most this big (× body). */
const TITLE_MAX_RATIO = 0.95
/** Gaps once the same line has been seen at the same place on a neighbouring page. */
const GAP_CONFIRMED = 0.4
const GAP_PAGE_NUMBER_CONFIRMED = 0.3
/** A lone number centred on the page, last on it: it only has to clear the line above. */
const GAP_PAGE_NUMBER_CENTRED = 0.05
/** A centred number in a typewriter face (AAAI) needs a clear berth: a listing's numbers do not sit there. */
const GAP_MONO_PAGE_NUMBER = 1.5
/** A page number this many em away from the nearest text of its row is "set apart". */
const APART_PAGE_NUMBER = 4
/** Horizontal tolerance (fraction of the page width) for "centred on the page". */
const CENTRED_TOL = 0.04
/** Longest running header / footer line, in glyphs (a journal footer carries a DOI URL). */
const MAX_RUNNING_GLYPHS = 100
const MAX_RUNNING_GLYPHS_STRIP = 130
const MAX_RUNNING_GLYPHS_CONFIRMED = 200
/** A journal footer has up to four parts: title | volume | DOI | number. */
const MAX_RUNNING_SEGS = 4
/** Running heads are set a little smaller than, or as, the body — inside the outer strip
 *  they may be much smaller (browser print-outs and slide decks put a 8 px URL line under
 *  12 px text). */
const RUNNING_FS: [number, number] = [0.7, 1.2]
const RUNNING_FS_STRIP: [number, number] = [0.5, 1.25]
const RUNNING_FS_CONFIRMED: [number, number] = [0.5, 1.6]
/** ...and a lone mark in front of one at least this small (× body). */
const FOOTNOTE_FRAGMENT_MIN = 0.35
/** Footnotes are set clearly smaller than the body but not tiny (figure labels are). */
const FOOTNOTE_FS: [number, number] = [0.55, 0.925]
/** At or above this ratio a segment counts as body-size text. */
const BODY_FS_MIN = 0.93
/** Footnote lines closer than this (em, edge to edge) belong to one block. Lines of one note stand
 *  0.2–0.4 em apart and consecutive notes 0.5; a display equation that ends the text sits 0.8 em or more
 *  above the first note, which keeps it out of the block. */
const FOOTNOTE_LINE_GAP = 0.7
/** A footnote block with a mark can be long; one without must be a short notice. */
const FOOTNOTE_MAX_ROWS_MARKED = 18
/** A line that carries a footnote mark has this many glyphs at least (a note, not a label). */
const FOOTNOTE_MARKED_LINE_GLYPHS = 12
/** A line this close to its column's left edge (em) is flush with it. */
const FOOTNOTE_FLUSH = 0.7
/** A block this close under an accepted one (em) belongs to the same footnote area. */
const FOOTNOTE_REGION_GAP = 1.3
const FOOTNOTE_MAX_ROWS_PLAIN = 4
/** A note without a mark is a notice: a line or two, as wide as the text it sits under. */
const FOOTNOTE_PLAIN_MIN_GLYPHS = 20
const FOOTNOTE_PLAIN_MIN_WIDTH = 0.5
const FOOTNOTE_PLAIN_MIN_BOTTOM = 0.88
/** Smallest gap (em) between the text and a footnote block that starts with a mark / has none. */
const FOOTNOTE_GAP_MARKED = 0.1
const FOOTNOTE_GAP_PLAIN = 1.0
/** Landscape pages set in type this large are slides: no flow, no footnotes. */
const SLIDE_MIN_FONT = 14
/** A segment with fewer glyphs than this is a fragment, not a line of text. */
const FOOTNOTE_MIN_LINE_GLYPHS = 4
/** Footnote lines on one row closer than this (em) are one note (or one table row). */
const FOOTNOTE_ROW_GAP = 3
const FOOTNOTE_MAX_HEIGHT = 0.3
/** The last line of a footnote block lies this far down the page at least (fraction of H):
 *  LaTeX and Word both push footnotes to the foot of the page, even a half-empty one. */
const FOOTNOTE_MIN_BOTTOM = 0.8
/** A footnote block starts at the body's left edge plus a paragraph indent (em). */
const FOOTNOTE_INDENT = 2
/** Rotated text whose centre lies in this outer fraction of the page is margin stamp. */
const MARGIN_X = 0.07
const MARGIN_Y = 0.04

/** Three letters in a row: the least a header / footer label has, and a formula never does. */
const WORD_RUN = /\p{L}{3}/u

// ═══════════════════════════════════════════════════════════════════════════════
// Neighbour pages: "the same line, in the same place"
// ═══════════════════════════════════════════════════════════════════════════════

interface EdgeSig {
  head: boolean
  norm: string
  /** Distance from the page edge as a fraction of the page height. */
  d: number
  num: boolean
  /** Horizontal centre as a fraction of the page width. */
  xc: number
  /** The bare numbers the row consists of or carries (a page number changes from page to page). */
  values: string[]
}

function edgeSignatures(page: FurniturePage): EdgeSig[] {
  const lay = buildLayout(page)
  const H = page.height
  const out: EdgeSig[] = []
  const n = lay.rows.length
  for (let k = 0; k < Math.min(3, n); k++) {
    for (const head of [true, false]) {
      const r = lay.rows[head ? k : n - 1 - k]
      const d = head ? r.cy / H : 1 - r.cy / H
      if (d > 0.2) continue
      out.push({
        head, norm: normalizeRepeat(r.text), d, num: r.segs.length === 1 && isPageNumberText(r.text),
        xc: (r.segs[0].x0 + r.segs[r.segs.length - 1].x1) / 2 / page.width,
        values: r.segs.filter(sg => isPageNumberText(sg.text, true)).map(sg => squash(sg.text)),
      })
    }
  }
  return out
}

/** Same side, same text modulo digits (or both a lone number at about the same x — a page
 *  number is not a column of axis ticks), and within ~1.2% of the page height of the same
 *  distance from the edge. */
/** A neighbour page carries the very same number at the same place: whatever this is, it is not a page
 *  number (the "1" of a figure panel, the "3" of a table that runs on). */
function sameNumberOnNeighbour(sigs: EdgeSig[], head: boolean, d: number, values: string[]): boolean {
  if (values.length === 0) return false
  for (const s of sigs) {
    if (s.head !== head || Math.abs(s.d - d) > 0.012) continue
    if (s.values.some(v => values.includes(v))) return true
  }
  return false
}

function seenOnNeighbour(sigs: EdgeSig[], head: boolean, norm: string, d: number, num: boolean, xc = 0): boolean {
  for (const s of sigs) {
    if (s.head !== head || Math.abs(s.d - d) > 0.012) continue
    if (num ? s.num && Math.abs(s.xc - xc) <= 0.06 : s.norm === norm && norm.length > 0) return true
  }
  return false
}

// ═══════════════════════════════════════════════════════════════════════════════
// Classification
// ═══════════════════════════════════════════════════════════════════════════════

interface Ctx {
  page: FurniturePage
  /** A landscape page set in large type: a slide. Its top line is a title, never a running head. */
  slide: boolean
  lay: Layout
  /** Body font size, px at scale 1. */
  B: number
  segKind: Array<FurnitureKind | null>
  /** The float (index into the page's captions) every segment of kind 'float' belongs to, -1 for the rest. */
  segFloat: Int32Array
  sigs: EdgeSig[]
}

/** The label (not the page number) of a row sits in the middle of the page and is narrow: a
 *  running title. The first line of a paragraph is flush with the margin and fills the column. */
function isCentredLabel(r: Row, nums: boolean[], pageWidth: number): boolean {
  let widest: Seg | null = null
  r.segs.forEach((sg, i) => { if (!nums[i] && (!widest || sg.x1 - sg.x0 > widest.x1 - widest.x0)) widest = sg })
  if (!widest) return false
  const w = (widest as Seg).x1 - (widest as Seg).x0
  return Math.abs(((widest as Seg).x0 + (widest as Seg).x1) / 2 - pageWidth / 2) <= 0.05 * pageWidth && w <= 0.75 * pageWidth
}

/** A separator drawn between the parts of a running head ("|", "·", "–"). */
const SEPARATOR = /^[|·•‧∙–—\-/\\]+$/

/** Whether some page-number segment of the row is set well apart from the text beside it: the
 *  page number of a running head sits at the far end of the line, a section number touches its title. */
function numberApart(r: Row, nums: boolean[]): boolean {
  return nums.some((isNum, i) => {
    if (!isNum) return false
    const me = r.segs[i]
    let best = Infinity
    r.segs.forEach((sg, j) => {
      if (j === i || SEPARATOR.test(sg.text)) return
      const g = j > i ? sg.x0 - me.x1 : me.x0 - sg.x1
      if (g < best) best = g
    })
    return best >= APART_PAGE_NUMBER * me.fs
  })
}

/**
 * Kind of every segment of `r` when the row is furniture of the given edge, else null.
 *   gap     white space (px) between the row and the next row towards the page centre
 *   order   how many furniture rows were already found further out on this edge
 *   strip   demand the outer strip even for a confirmed line (the pair rule)
 */
function evalEdgeRow(ctx: Ctx, r: Row, head: boolean, gap: number, order: number, strip = false): FurnitureKind[] | null {
  const { B, page, sigs } = ctx
  const d = head ? r.cy / page.height : 1 - r.cy / page.height
  if (d > EDGE_ZONE_CONFIRMED) return null
  const allNums = r.segs.every(s => isPageNumberText(s.text))
  // Next to text, only a plain number counts as a page number: "E.4" in front of a heading is a
  // section number.
  const nums = allNums ? r.segs.map(() => true) : r.segs.map(s => isPageNumberText(s.text, true))
  if (sigs.length > 0 && sameNumberOnNeighbour(sigs, head, d, r.segs.filter((_, i) => nums[i]).map(s => squash(s.text)))) return null

  if (allNums && r.segs.length <= 3) {
    // Several numbers on one line are the pages of a spread ("2" and "3"), not two ticks of an axis.
    if (r.segs.length > 1) {
      const v = r.segs.map(sg => /^\d{1,4}$/.test(squash(sg.text)) ? Number(squash(sg.text)) : NaN)
      if (v.some(x => Number.isNaN(x)) || v.some((x, i) => i > 0 && x !== v[i - 1] + 1)) return null
    }
    // A line that is only a page number. Isolated, outermost, not oversized, not a line
    // number of a listing, not one of a column of axis ticks.
    // Centred on the page like most page numbers are: the one placement where a number needs
    // hardly any white space above it to be told from a line of the text (the ACL anthology and
    // Word set it a third of an em under the last line), and where a typewriter face is a
    // style choice rather than a listing's line number (AAAI).
    const centred = r.segs.length === 1 && isBareNumber(r.segs[0].text) &&
      Math.abs((r.segs[0].x0 + r.segs[0].x1) / 2 - page.width / 2) <= CENTRED_TOL * page.width
    if (r.fs > 1.5 * B || d > EDGE_PAGE_NUMBER) return null
    if (r.mono > 0.5 && !(centred && gap >= GAP_MONO_PAGE_NUMBER * B)) return null
    if (d > EDGE_ZONE && gap < 1.5 * B) return null
    // Away from the outer strip a number is one of the text block's own unless it is set like a page number.
    if (d > EDGE_STRIP && r.fs < 0.8 * B) return null
    const inner = ctx.lay.rows[head ? r.segs[0].row + 1 : r.segs[0].row - 1]
    if (inner && inner.segs.length === 1 && NUMERIC_ONLY.test(inner.text) &&
        Math.abs((inner.segs[0].x0 + inner.segs[0].x1) / 2 - (r.segs[0].x0 + r.segs[r.segs.length - 1].x1) / 2) < 0.02 * page.width &&
        gap < 3 * B) return null
    const seen = r.segs.length === 1 && seenOnNeighbour(sigs, head, '', d, true, (r.segs[0].x0 + r.segs[0].x1) / 2 / page.width)
    // "x", "I", "x2", "Q1": a number only where the page says so (see isWeakPageNumberText).
    if (r.segs.length === 1 && isWeakPageNumberText(r.segs[0].text) && !seen &&
        !(d <= EDGE_STRIP && Math.abs((r.segs[0].x0 + r.segs[0].x1) / 2 - page.width / 2) <= CENTRED_TOL * page.width)) return null
    if (gap < Math.min(GAP_PAGE_NUMBER, seen ? GAP_PAGE_NUMBER_CONFIRMED : Infinity, centred ? GAP_PAGE_NUMBER_CENTRED : Infinity) * B) return null
    if (order > 0 && !seen && d > EDGE_STRIP) return null
    return r.segs.map(() => 'pageNumber' as const)
  }

  // A running header / footer: a short label (maybe next to a page number) set apart from the body.
  if (r.segs.length > MAX_RUNNING_SEGS || r.mono > 0.5) return null
  const label = r.segs.filter((_, i) => !nums[i]).map(sg => sg.text).join(' ')
  const seen = seenOnNeighbour(sigs, head, normalizeRepeat(r.text), d, false)
  const inStrip = d <= EDGE_STRIP
  if (r.glyphs > (seen ? MAX_RUNNING_GLYPHS_CONFIRMED : inStrip ? MAX_RUNNING_GLYPHS_STRIP : MAX_RUNNING_GLYPHS)) return null
  if (!WORD_RUN.test(label)) return null
  const [lo, hi] = seen ? RUNNING_FS_CONFIRMED : inStrip ? RUNNING_FS_STRIP : RUNNING_FS
  const ratio = r.fs / B
  if (ratio < lo || ratio > hi) return null
  if (strip && !inStrip) return null
  // What names a line as a running head. A slide's credit line ("Qiu et al.", a link to the code) says nothing: there it
  // takes a neighbour that repeats it or the very edge of the page.
  const names = RUNNING_WORDS.test(label) ||
    ((isAuthorRunningHead(label) || isUrlLabel(label)) && (!ctx.slide || seen || d <= EDGE_HAIRLINE))
  // A centred running title is set a little smaller than the body; a centred line of the
  // text itself (a list of names, a quotation) is not.
  const title = d <= EDGE_STRIP && ratio <= TITLE_MAX_RATIO && isCentredLabel(r, nums, page.width)
  if (seen) {
    // A neighbour repeating the line only shows that it is templated, not that it is furniture:
    // figure captions of an appendix, the column headings of a table that runs on, and the
    // titles of consecutive slides repeat just as faithfully. Away from the outer strip only a
    // line that names itself ("Manuscript submitted to ACM") or carries a page number is taken.
    if (isContentLabel(label) || CONTINUED.test(label)) return null
    if (!inStrip && !names && !nums.some(Boolean)) return null
    if (r.segs.filter((_, i) => !nums[i]).length >= 3 && !names) return null
    if (ctx.slide && head) return null
  }
  if (!seen) {
    if (d > EDGE_ZONE) return null
    // What is set in smaller type at the edge of a page of text but is part of it: a caption, a heading
    // of the paper's own structure.
    if (isContentLabel(label) || SECTION_HEADING.test(label.trim())) return null
    // (A venue notice — "Proceedings of the 63rd Annual Meeting of the Association for …, pages
    // 31842–31856" — is as full of function words as a sentence is; what it lacks is the full stop.)
    if (looksLikeProse(label) && !(names && /^\p{Lu}/u.test(label) && !/[.!?:;,]$/.test(label.trim()))) return null
    // Outside the outer strip a line needs a much wider berth: the last / first line of a
    // full page of text lies in the zone too. And a second text line that no other page
    // vouches for is never taken on faith.
    if (order > 0) return null
    // Between the hairline and the page's text area a first / last line of text is just as
    // alone as a header: only a label that says what it is, or that carries a page number,
    // is taken — and outside the strip that label needs the wide berth unless it says so.
    const says = names || nums.some(Boolean)
    if (d > EDGE_HAIRLINE && !says && !title) return null
    // A line at the body size or above that no other page repeats and that does not say what it
    // is, is the first / last line of the text (a web page printed with its margins at the
    // very edge, a section heading): running heads are set smaller or are vouched for.
    // (A number beside it does not vouch for it: "5 RELATED WORK" is a section heading — unless
    // the number stands well apart from the text, at the far end of the line.)
    if (ratio > UNCONFIRMED_MAX_RATIO && !names && !numberApart(r, nums)) return null
    if (!inStrip && gap < (says ? GAP_RUNNING : GAP_RUNNING_WIDE) * B) return null
  }
  if (gap < (seen ? GAP_CONFIRMED : d <= EDGE_HAIRLINE ? (names ? GAP_RUNNING_HAIRLINE_NAMED : GAP_RUNNING_HAIRLINE) : names || title ? GAP_RUNNING_TITLE : GAP_RUNNING) * B) return null
  const kind: FurnitureKind = head ? 'header' : 'footer'
  return r.segs.map((_, i) => (nums[i] ? 'pageNumber' : kind))
}

/** Walk inwards from one page edge and flag the rows that are furniture. */
function classifyEdge(ctx: Ctx, head: boolean): void {
  const rows = ctx.lay.rows
  const n = rows.length
  const at = (k: number): Row | undefined => (head ? rows[k] : rows[n - 1 - k])
  const gapBetween = (outer: Row, inner: Row | undefined): number =>
    inner === undefined ? Infinity : head ? inner.top - outer.bottom : outer.top - inner.bottom
  const apply = (r: Row, kinds: FurnitureKind[]) => r.segs.forEach((sg, i) => { ctx.segKind[sg.id] = kinds[i] })

  let k = 0
  let accepted = 0
  while (k < n && accepted < 3) {
    const r = at(k)!
    const kinds = evalEdgeRow(ctx, r, head, gapBetween(r, at(k + 1)), accepted)
    if (kinds) { apply(r, kinds); accepted++; k++; continue }
    // A two-line header / footer: the pair is judged against what lies inside it.
    const r2 = at(k + 1)
    if (!r2 || accepted > 0) break
    if (gapBetween(r, r2) > 2.5 * ctx.B) break
    const g = gapBetween(r2, at(k + 2))
    const k1 = evalEdgeRow(ctx, r, head, g, 0, true)
    const k2 = k1 && evalEdgeRow(ctx, r2, head, g, 0, true)
    if (k1 && k2) { apply(r, k1); apply(r2, k2); accepted += 2; k += 2; continue }
    break
  }
}

function overlapX(a: Seg, b: Seg): number {
  return Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0)
}

/** Whether two segments stand one above the other (share a good part of their x range). */
function stacked(a: Seg, b: Seg): boolean {
  const ov = overlapX(a, b)
  return ov > 0 && ov >= 0.3 * Math.min(a.x1 - a.x0, b.x1 - b.x0)
}

/** What follows a raised digit that makes it a footnote mark rather than an isotope, an exponent or a
 *  label: any text but another number, unless it is a lone capital letter followed by more capitals or a
 *  sign ("13C NMR", "1H + 13C NMR" are not notes; "A significant", "M was set to 5" are). */
function noteFollows(text: string): boolean {
  const t = text.trim()
  if (t.length === 0 || /^\d/.test(t)) return false
  return !/^\p{L}(?:\s+[\p{Lu}+=\-−/]|\s*$)/u.test(t)
}

function startsWithFootnoteMark(ctx: Ctx, seg: Seg, letters = true): boolean {
  const spans = ctx.page.spans
  const solid = seg.spans.filter(i => !isBlank(spans[i].text))
  if (solid.length === 0) return false
  const first = spans[solid[0]]
  // A raised, smaller fragment in front of the note: "¹Equal contribution", "∗Work done while …" —
  // whatever face the producer put it in (the star of a math font is reported as typewriter).
  if (solid.length >= 2 && (!/^\d/.test(first.text) || noteFollows(seg.text.replace(/^\s*\d{1,2}\s*/, ''))) && (letters ? /^(?:\d{1,2}|[a-z]|[*∗†‡§¶‖⋆★✝✠※]+)$/u : /^(?:\d{1,2}|[*∗†‡§¶‖⋆★✝✠※]+)$/u).test(first.text.trim()) &&
      first.fontSize < SUPERSCRIPT_RATIO * spans[solid[1]].fontSize) return true
  // In typewriter type only a digit glued to the text is taken ("2https://…"): a "**" of
  // markdown, a "#" of code or a "1  import os" line number is not a footnote mark.
  if (first.mono) return /^\d{1,2}(?=\p{L})/u.test(seg.text)
  return FOOTNOTE_MARK.test(seg.text)
}

/**
 * A line of words that justification pulled apart (an unbreakable URL leaves the rest of its line with
 * gaps of 2–3 em), as opposed to a row of table cells: words in every piece, and one gap width throughout
 * (cells are as wide as their columns, so the gaps between them vary). The short words of the line that
 * are not pieces of it (`fragments`) are counted in.
 */
function justifiedRow(pieces: Seg[], fragments: Seg[], B: number): boolean {
  const row = pieces[0].row
  const all = [...pieces, ...fragments.filter(f => f.row === row && f.x0 > Math.min(...pieces.map(p => p.x0)) && f.x1 < Math.max(...pieces.map(p => p.x1)))]
    .sort((a, b) => a.x0 - b.x0)
  if (!all.every(sg => /\p{L}/u.test(sg.text))) return false
  const gaps: number[] = []
  for (let i = 1; i < all.length; i++) gaps.push(all[i].x0 - all[i - 1].x1)
  if (gaps.some(g => g < 0 || g > FOOTNOTE_ROW_GAP * B)) return false
  return Math.max(...gaps) - Math.min(...gaps) <= 0.25 * B
}

/** Rows whose centres coincide while their left edges do not: a centred caption. */
function isCentered(rowsOf: Map<number, Seg[]>, B: number): boolean {
  let cMin = Infinity, cMax = -Infinity, lMin = Infinity, lMax = -Infinity
  for (const ss of rowsOf.values()) {
    const x0 = ss.reduce((m, s) => Math.min(m, s.x0), Infinity)
    const x1 = ss.reduce((m, s) => Math.max(m, s.x1), -Infinity)
    const c = (x0 + x1) / 2
    cMin = Math.min(cMin, c); cMax = Math.max(cMax, c)
    lMin = Math.min(lMin, x0); lMax = Math.max(lMax, x0)
  }
  return cMax - cMin <= 0.5 * B && lMax - lMin >= 1.2 * B
}

function classifyFootnotes(ctx: Ctx): void {
  const { lay, B, page, segKind } = ctx
  const segs = lay.segs
  const body: Seg[] = []
  const cand: Seg[] = []
  const rest: Seg[] = []
  for (let i = 0; i < segs.length; i++) {
    if (segKind[i] !== null) continue
    const s = segs[i]
    const ratio = s.fs / B
    if (ratio >= BODY_FS_MIN) body.push(s)
    else if (ratio >= FOOTNOTE_FS[0] && ratio <= FOOTNOTE_FS[1]) cand.push(s)
    // A raised mark can be much smaller than the note it stands in front of (5 px against 9).
    else if (ratio >= FOOTNOTE_FRAGMENT_MIN && ratio < FOOTNOTE_FS[0] && s.glyphs < FOOTNOTE_MIN_LINE_GLYPHS) cand.push(s)
    rest.push(s)
  }
  if (cand.length === 0 || body.length === 0) return

  // A candidate is TAIL when no body-size text lies below it in its own column, and it
  // has body-size text above it to be a footnote OF. Both are what separates a footnote
  // from a caption or a table in the middle of the flow, or from a references page.
  const tail: Seg[] = []
  for (const c of cand) {
    let blocked = false
    let anchored = false
    for (const b of body) {
      if (!stacked(c, b)) continue
      if (b.cy > c.cy + 0.5 * (c.y1 - c.y0)) { blocked = true; break }
      if (b.cy < c.cy && b.glyphs >= 8) anchored = true
    }
    if (!blocked && anchored) tail.push(c)
  }
  // Fragments (a subscript, a lone mark) do not make a note: they follow the lines they sit on.
  const fragments = tail.filter(c => c.glyphs < FOOTNOTE_MIN_LINE_GLYPHS)
  const lines = tail.filter(c => c.glyphs >= FOOTNOTE_MIN_LINE_GLYPHS)
  if (lines.length === 0) return
  tail.length = 0
  tail.push(...lines)
  tail.sort((a, b) => a.y0 - b.y0 || a.x0 - b.x0)

  // Chain footnote lines into blocks: lines that stand one under the other, and segments
  // that sit on one row close together (a note wrapped across a justified gap — but also a
  // table row, a figure's labels, a two-column caption: those fail the checks below).
  const parent = tail.map((_, i) => i)
  const find = (i: number): number => { while (parent[i] !== i) { parent[i] = parent[parent[i]]; i = parent[i] } return i }
  // Two pieces of one row are one note when the gap between them is short, or is spanned by short words
  // ("and", "at") a justified line stretched apart — an unbreakable URL leaves gaps of 2–3 em between
  // every word of its line, and each word that is too short to count as a line by itself.
  const bridged = (a: Seg, b: Seg): boolean => {
    const [l, r] = a.x0 <= b.x0 ? [a, b] : [b, a]
    let right = l.x1
    for (const f of fragments) {
      if (f.row !== l.row || f.x0 < right || f.x1 > r.x0) continue
      if (f.x0 - right >= FOOTNOTE_ROW_GAP * B) return false
      right = f.x1
    }
    return r.x0 - right < FOOTNOTE_ROW_GAP * B
  }
  for (let i = 0; i < tail.length; i++) {
    const a = tail[i]
    for (let j = i + 1; j < tail.length; j++) {
      const b = tail[j]
      if (b.y0 - a.y1 > FOOTNOTE_LINE_GAP * B) break
      const sameRow = Math.abs(a.cy - b.cy) <= 0.5 * Math.max(a.y1 - a.y0, b.y1 - b.y0)
      const linked = sameRow
        ? Math.max(a.x0, b.x0) - Math.min(a.x1, b.x1) < FOOTNOTE_ROW_GAP * B || (a.row === b.row && bridged(a, b))
        : Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0) > 0
      if (linked) parent[find(j)] = find(i)
    }
  }
  interface Block { segs: Seg[]; x0: number; x1: number; top: number; bottom: number }
  const byRoot = new Map<number, Block>()
  tail.forEach((c, i) => {
    const r = find(i)
    const bl = byRoot.get(r)
    if (bl) {
      bl.segs.push(c)
      bl.x0 = Math.min(bl.x0, c.x0); bl.x1 = Math.max(bl.x1, c.x1)
      bl.top = Math.min(bl.top, c.y0); bl.bottom = Math.max(bl.bottom, c.y1)
    } else byRoot.set(r, { segs: [c], x0: c.x0, x1: c.x1, top: c.y0, bottom: c.y1 })
  })

  // The column's left edge at some x range: where most of its body-size text starts (a table row
  // or a wide line that happens to cross the column must not move it).
  const columnEdges = (x0: number, x1: number, above: number): { left: number; right: number; heading: boolean; above: number } => {
    let heading = false
    let top = -Infinity
    let right = -Infinity
    const edges = new Map<number, { x0: number; glyphs: number }>()
    for (const s of rest) {
      if (!(Math.min(s.x1, x1) - Math.max(s.x0, x0) > 0)) continue
      if (s.y1 <= above) {
        if (NOTES_HEADING.test(s.text)) { heading = true; break }
        // Fragments (a limit under a sum sign, a lone mark) are part of a line, not text above.
        if (s.glyphs >= FOOTNOTE_MIN_LINE_GLYPHS && s.y1 > top) top = s.y1
      }
      if (s.fs / B >= BODY_FS_MIN && s.glyphs >= 25) {
        const k = Math.round(s.x0 / (0.5 * B))
        const e = edges.get(k)
        if (e) { e.glyphs += s.glyphs; if (s.x0 < e.x0) e.x0 = s.x0 } else edges.set(k, { x0: s.x0, glyphs: s.glyphs })
        if (s.x1 > right) right = s.x1
      }
    }
    let left = Infinity
    let leftGlyphs = 0
    for (const e of edges.values()) if (e.glyphs > leftGlyphs || (e.glyphs === leftGlyphs && e.x0 < left)) { left = e.x0; leftGlyphs = e.glyphs }
    return { left, right, heading, above: top }
  }
  // The mark may be a separate fragment just before a line (a raised digit set off by a justified
  // gap), which is no part of that line's text.
  const markBefore = (sg: Seg) => fragments.some(f =>
    (SYMBOL_MARK.test(f.text) || (FRAGMENT_MARK.test(f.text) && f.fs < SUPERSCRIPT_RATIO * sg.fs)) &&
    f.x1 <= sg.x0 + 0.5 * B && sg.x0 - f.x1 < 2 * B && Math.abs(f.cy - sg.cy) <= 0.8 * (sg.y1 - sg.y0))

  // Blocks from the top of the page down: a note without a mark of its own (a venue notice under
  // the author footnotes) is accepted when it directly continues a block that was.
  const blocks = [...byRoot.values()].sort((a, b) => a.top - b.top)
  // A footnote AREA is a run of blocks that follow each other closely (the author notes, the
  // permission notice and the venue line of a first page). It is the area that has to lie at the
  // foot of the page and must not be a page-sized pile of small print, not the single block.
  const area = blocks.map(() => ({ top: Infinity, bottom: -Infinity }))
  blocks.forEach((bl, i) => {
    let lo = i
    let hi = i
    while (lo > 0 && blocks[lo].top - blocks[lo - 1].bottom <= FOOTNOTE_REGION_GAP * B &&
           Math.min(blocks[lo].x1, blocks[lo - 1].x1) - Math.max(blocks[lo].x0, blocks[lo - 1].x0) > 0) lo--
    while (hi < blocks.length - 1 && blocks[hi + 1].top - blocks[hi].bottom <= FOOTNOTE_REGION_GAP * B &&
           Math.min(blocks[hi].x1, blocks[hi + 1].x1) - Math.max(blocks[hi].x0, blocks[hi + 1].x0) > 0) hi++
    area[i] = { top: blocks[lo].top, bottom: blocks[hi].bottom }
  })
  const accepted: Block[] = []
  for (let bi = 0; bi < blocks.length; bi++) {
    const bl = blocks[bi]
    // At the foot of the page, and not a page-sized pile of small print.
    if (area[bi].bottom < FOOTNOTE_MIN_BOTTOM * page.height || area[bi].bottom - area[bi].top > FOOTNOTE_MAX_HEIGHT * page.height) continue

    const rowsOfAll = new Map<number, Seg[]>()
    for (const s of bl.segs) { const a = rowsOfAll.get(s.row); if (a) a.push(s); else rowsOfAll.set(s.row, [s]) }
    if (rowsOfAll.size > FOOTNOTE_MAX_ROWS_MARKED) continue

    let first = bl.segs.reduce((a, b) => (b.y0 < a.y0 - 1 || (Math.abs(b.y0 - a.y0) <= 1 && b.x0 < a.x0) ? b : a))
    if (bl.segs.some(s => CAPTION_START.test(s.text))) continue // a caption (or its figure's labels)
    if (bl.segs.some(s => NOTES_HEADING.test(s.text))) continue // the heading of a reference list

    const cols = columnEdges(first.x0, first.x1, bl.top + 0.5 * (first.y1 - first.y0))
    if (cols.heading) continue
    const blockLeft = bl.segs.reduce((m, s) => Math.min(m, s.x0), Infinity)
    if (isFinite(cols.left) && blockLeft > cols.left + FOOTNOTE_INDENT * B) continue

    // Where do the notes start? Any line at the column's edge that begins with a symbol / a digit
    // mark and carries a real sentence will do (a raised letter inside a formula never starts such
    // a line). Lines above the first of them that are not flush with their column — a display
    // equation that ends the text and sits closer than usual — are not part of the notes.
    const leftEdge = isFinite(cols.left) ? cols.left : blockLeft
    const firstMarked = startsWithFootnoteMark(ctx, first) || markBefore(first)
    const starts = bl.segs.filter(sg => sg.glyphs >= FOOTNOTE_MARKED_LINE_GLYPHS && sg.x0 <= leftEdge + FOOTNOTE_INDENT * B &&
      (startsWithFootnoteMark(ctx, sg, false) || markBefore(sg)))
    const marked = firstMarked || starts.length > 0
    let members = bl.segs
    if (!firstMarked && starts.length > 0) {
      const noteTop = Math.min(...starts.map(sg => sg.y0))
      members = bl.segs.filter(sg => sg.y0 >= noteTop - 1 || sg.x0 <= columnEdges(sg.x0, sg.x1, sg.y0).left + FOOTNOTE_FLUSH * B)
      if (members.length === 0) continue
      first = members.reduce((a, b) => (b.y0 < a.y0 - 1 || (Math.abs(b.y0 - a.y0) <= 1 && b.x0 < a.x0) ? b : a))
    }

    const rowsOf = new Map<number, Seg[]>()
    for (const s of members) { const a = rowsOf.get(s.row); if (a) a.push(s); else rowsOf.set(s.row, [s]) }
    const nrows = rowsOf.size
    const firstRowGlyphs = (rowsOf.get(first.row) ?? [first]).reduce((a, sg) => a + sg.glyphs, 0)
    if (firstRowGlyphs < 6) continue
    const text = members.map(s => s.text).join(' ')
    const glyphs = members.reduce((a, s) => a + s.glyphs, 0)
    if (glyphs < 10) continue
    let wide = 0
    for (const ss of rowsOf.values()) if (ss.length >= 3 && !justifiedRow(ss, fragments, B)) wide++
    if (wide >= 0.4 * nrows || digitRatio(text) > 0.4 || numericTokenRatio(text) > 0.5) continue // a table, not notes
    if (members.some(s => /^\[\d+\]/.test(s.text))) continue // a numbered reference list

    const mono = members.reduce((a, s) => a + s.mono * s.glyphs, 0) / glyphs
    if (!marked && mono > 0.5) continue // a code / prompt listing

    const blockTop = members.reduce((m, s) => Math.min(m, s.y0), Infinity)
    const blockBottom = members.reduce((m, s) => Math.max(m, s.y1), -Infinity)
    // A block that directly continues an accepted one belongs to the same footnote area.
    const continues = accepted.some(ab => blockTop - ab.bottom <= FOOTNOTE_REGION_GAP * B && blockTop >= ab.top &&
      Math.min(ab.x1, bl.x1) - Math.max(ab.x0, bl.x0) > 0)

    if (!marked && !continues) {
      if (nrows > FOOTNOTE_MAX_ROWS_PLAIN || glyphs < FOOTNOTE_PLAIN_MIN_GLYPHS || bl.bottom < FOOTNOTE_PLAIN_MIN_BOTTOM * page.height) continue
      const widest = members.reduce((m, s) => Math.max(m, s.x1 - s.x0), 0)
      if (!(cols.right > cols.left) || widest < FOOTNOTE_PLAIN_MIN_WIDTH * (cols.right - cols.left)) continue
      if (!NOTICE_WORDS.test(first.text)) continue
    }

    // Notes hang under a rule and a gap; text that merely continues the column in a
    // smaller size (a list, a quotation, a paragraph of mixed fonts) sits at normal leading.
    const gapAbove = isFinite(cols.above) ? blockTop - cols.above : Infinity
    if (!continues && gapAbove < (marked ? FOOTNOTE_GAP_MARKED : FOOTNOTE_GAP_PLAIN) * B) continue
    // Lines centred under each other are a caption, left-aligned ones are notes.
    if (nrows >= 2 && isCentered(rowsOf, B)) continue
    for (const s of members) segKind[s.id] = 'footnote'
    accepted.push({ segs: members, x0: bl.x0, x1: bl.x1, top: blockTop, bottom: blockBottom })
    for (const f of fragments) {
      for (const l of members) {
        if (Math.abs(f.cy - l.cy) <= 0.5 * Math.max(f.y1 - f.y0, l.y1 - l.y0) && Math.max(f.x0, l.x0) - Math.min(f.x1, l.x1) < FOOTNOTE_ROW_GAP * B) { segKind[f.id] = 'footnote'; break }
      }
    }
  }
}

/** Rotated text in the page margin: arXiv stamps, side watermarks, running titles. */
function classifyMargin(page: FurniturePage, out: Array<FurnitureKind | null>): void {
  const spans = page.spans
  const W = page.width
  const H = page.height
  for (let i = 0; i < spans.length; i++) {
    const s = spans[i]
    if (isHorizontal(s) || isBlank(s.text)) continue
    const cx = s.x + s.w / 2
    const cy = s.y + s.h / 2
    if (cx <= MARGIN_X * W || cx >= (1 - MARGIN_X) * W || cy <= MARGIN_Y * H || cy >= (1 - MARGIN_Y) * H) out[i] = 'margin'
  }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Floats: figures, tables, algorithms — their captions and the text they hold
// ═══════════════════════════════════════════════════════════════════════════════
//
// A figure or a table is set wherever it fits — the top or the foot of a page, between two
// paragraphs — and its text (the caption, a table's cells, a chart's labels) lies across the flow
// of the running text: a drag from the foot of one page into the next selects the float at the top
// of the second as part of the passage. A float is found from its CAPTION — a block that begins with
// a label, a number and a delimiter ("Figure 3:", "Fig. 2.", "Table 1", "TABLE IV", "Algorithm 1 …",
// "图 3", "表1：") and that does not continue the paragraph above it — and then from the caption
// outwards: a figure's labels above it, a table's rows on whichever side holds rows of cells, up to
// the first line that could belong to the running text. Every rule leans towards finding less: a
// caption that cannot be told from a line of the text is not one, and the walk stops at the first
// line that could be the text.

type FloatType = 'figure' | 'table' | 'algorithm' | 'listing'

/** "Figure 3", "Fig. 2", "Figs 3", "Table 1", "TABLE IV", "Table A.1", "Figure S3", "Algorithm 2", "Listing 1",
 *  "Supplementary Fig. 3", "Extended Data Table 1" — and the label in the languages a paper is written in
 *  (Abb. / Abbildung / Tabelle, Tableau / Graphique, Tabla / Cuadro / Gráfico, Рис. / Таблица). The number must
 *  end the word: "Table3presents" (a PDF without spaces) is not a label. */
const CAPTION_HEAD = new RegExp(
  '^(?:(?:supplementary|extended\\s*data|source\\s*data|supporting)\\s*)?' +
  '(?:(fig(?:ure|ura|ur)?s?|abb(?:ildung)?|рис(?:унок)?|graphique|gr[aá]fico|plate|scheme|sch[eé]ma|exhibit)' +
  '|(tables?|tab|tabelle|tableau|tabla|tabella|cuadro|таблица|табл)' +
  '|(algorithm|alg|algorithme|algoritmo)' +
  '|(listing))' +
  '(?![\\p{L}])\\.?\\s*' +
  '((?:[A-Z]\\.?)?\\d{1,3}(?:[.\\-–]\\d{1,3})*[a-z]?(?![\\p{L}\\p{N}])|[IVXL]{1,6}(?![\\p{L}\\p{N}]))',
  'iu',
)
/** 图 3 / 圖3 / 図 3 / 그림 3 (figure), 表 1 / 표 1 (table), with the 附 / 补充 of a supplement. */
const CJK_CAPTION_HEAD = /^(?:附|补充|補充)?(?:([图圖図]|그림)|(表|표))\s*((?:[A-Z]|S)?\d{1,3}(?:[.\-－．]\d{1,3})*)(?![\p{N}])/u

type CaptionStrength = 'strong' | 'dot' | 'alone' | 'word'

interface CaptionHead {
  type: FloatType
  /** strong: "Figure 3:" / "Fig. 1 |"; dot: "Fig. 3. …"; alone: the label is all there is ("TABLE IV");
   *  word: "Fig. 1 Overview" — a capital after the number and no delimiter. */
  strength: CaptionStrength
  /** Length of the label (and of its delimiter, if any) at the start of the trimmed text. */
  labelEnd: number
}

/** Leader dots before a page number: an entry of a list of figures, not a caption. */
const LEADER = /(?:\.\s*){4,}\s*\d+\s*$|…\s*\d+\s*$/

/** Whether `text` starts like a caption, and how firmly. */
export function captionHead(text: string): CaptionHead | null {
  const t = text.replace(/^\s+/, '')
  let type: FloatType
  let end: number
  const m = CAPTION_HEAD.exec(t)
  if (m) {
    type = m[1] ? 'figure' : m[2] ? 'table' : m[3] ? 'algorithm' : 'listing'
    end = m[0].length
  } else {
    const c = CJK_CAPTION_HEAD.exec(t)
    if (!c) return null
    type = c[1] ? 'figure' : 'table'
    end = c[0].length
  }
  if (LEADER.test(t)) return null
  const rest = t.slice(end)
  const r = rest.replace(/^\s+/, '')
  const skipped = rest.length - r.length
  if (r === '') return { type, strength: 'alone', labelEnd: end }
  // (A dash only as a separator between spaces: "Figures 4a–4d present …" is a range.)
  if (/^[:|：︰]/u.test(r) || (skipped > 0 && /^[—–]\s/u.test(r))) return { type, strength: 'strong', labelEnd: end + skipped + 1 }
  // "Fig. 3. The …" is a caption, "… as shown in Figure 7." wrapped onto a line of its own is not.
  if (/^[.．。]/u.test(r)) return r.slice(1).trim() === '' ? null : { type, strength: 'dot', labelEnd: end + skipped + 1 }
  // "Fig. 1 Overview of …", "Algorithm 1 Training", "图 1 系统结构": no delimiter, a capital (or an ideograph)
  // after a space. The caller also wants the label set apart from the text (a span of its own).
  if (skipped > 0 && /^[\p{Lu}\p{Lo}]/u.test(r)) return { type, strength: 'word', labelEnd: end }
  return null
}

/** Set in a typewriter face: code, a prompt, a URL. (pdf.js reports many CJK faces as monospace too:
 *  ideographs are all one width. Those are text.) */
function isCodeSeg(sg: Seg): boolean {
  return sg.mono > 0.5 && !CJK.test(sg.text)
}

/** Running text: a line of prose, as opposed to a label, a cell, a formula or a line of code. */
function isProseSeg(sg: Seg): boolean {
  const t = sg.text
  // A line of Chinese / Japanese text is long, or carries the punctuation of a sentence; a cell is neither.
  if (CJK.test(t)) return sg.glyphs >= 20 || (sg.glyphs >= 8 && /[。！？；，、：]/u.test(t))
  if (isCodeSeg(sg)) return false
  if (sg.glyphs < 18 || !/\p{Ll}/u.test(t)) return false
  const words = t.split(' ').filter(w => /\p{L}/u.test(w)).length
  // Some PDFs carry no spaces at all ("Theresultsshowthat…"): a long run of letters is prose too.
  return words >= 3 || sg.glyphs >= 30
}

/** An equation number at the end of a display: "(3)", "(2.1)", "(4b)". */
const EQUATION_NUMBER = /^\(\s*(?:[A-Z]\.)?\d{1,3}(?:\.\d{1,3})?[a-z]?\s*\)$/

/** Gap (em) between two lines of one caption, edge to edge. Caption leading is 0.05–0.3 em. */
const CAPTION_LINE_GAP = 0.75
/** A caption is at most this many lines. */
const CAPTION_MAX_LINES = 30
/** Lines of one caption differ in size by at most this share. */
const CAPTION_FS_TOL = 0.06
/** A caption's line that ends this close (em) to the caption's right edge is full: the caption goes on. */
const CAPTION_FULL_LINE = 1.5
/** The line above a caption that shares its size and sits closer than this (em) is the paragraph the
 *  "caption" continues ("… are reported in Table 7. Obviously, …"). */
const CAPTION_FLOW_GAP = 0.6
/** Inside a float, consecutive lines of text stand closer than this (em); a wider white band is the
 *  picture itself, or the end of the float. */
const FLOAT_ROW_GAP = 1.2
/** A caption at least this big (× body) is in the size of the text: it has to show its float, or be short
 *  (this many lines at most) and set off by this much white (em) from the text that follows. */
const CAPTION_TEXT_SIZE = 0.95
const CAPTION_SHORT_LINES = 4
const CAPTION_SET_OFF = 0.9
/** The least white band (em) above a figure's caption that is taken for the figure's picture. */
const PICTURE_MIN_BAND = 2.5
/** Where a page's text area starts at the earliest (fraction of the page height): a 1 in margin is 0.09
 *  of a Letter page, 25 mm 0.084 of an A4 one. */
const TEXT_AREA_TOP = 0.08
/** A table may stand this far (em) from its caption. */
const TABLE_CAPTION_GAP = 2.0
/** Below this size (× body) a line under a picture is still the figure's (axis labels, a legend). */
const FIGURE_SMALL = 0.88
/** A heading of the text: at least this big (× body). */
const HEADING_MIN = 1.1

interface FloatBlock {
  type: FloatType
  segs: Seg[]
  /** First and last row of the caption. */
  top: number
  bottom: number
  x0: number
  x1: number
}

/** The segments of a row that belong to the column [x0, x1]: mostly inside it, or across most of it (a
 *  line of the text that runs past a narrow float). */
function segsWithin(r: Row, x0: number, x1: number): Seg[] {
  return r.segs.filter(sg => {
    const ov = Math.min(sg.x1, x1) - Math.max(sg.x0, x0)
    return ov >= 0.5 * Math.max(sg.x1 - sg.x0, 1e-6) || ov >= 0.5 * (x1 - x0)
  })
}

/** Whether the label of a caption ("Fig. 1") ends where a span ends: set in a face of its own. */
function labelOwnSpan(page: FurniturePage, sg: Seg, labelEnd: number): boolean {
  let n = 0
  const want = squash(sg.text).slice(0, labelEnd).replace(/\s+/g, '').length
  for (const i of sg.spans) {
    const t = page.spans[i].text.replace(/\s+/g, '')
    if (t === '') continue
    n += t.length
    if (n === want) return true
    if (n > want) return false
  }
  return false
}

/**
 * The column a caption sits in: the x range of the body lines that run past its centre, or the caption's
 * own range when none does (a caption across both columns, a caption beside another float).
 */
function floatExtent(ctx: Ctx, bl: FloatBlock): [number, number] {
  const { B } = ctx
  const w = bl.x1 - bl.x0
  const cx = (bl.x0 + bl.x1) / 2
  // A short caption centred under its float says nothing about the float's width: take the column's.
  const lines = new Set(bl.segs.map(s => s.row)).size
  if (lines === 1) {
    const x0s: number[] = []
    const x1s: number[] = []
    for (const sg of ctx.lay.segs) {
      if (ctx.segKind[sg.id] !== null || sg.fs < BODY_FS_MIN * B || !isProseSeg(sg)) continue
      if (sg.x0 <= cx && sg.x1 >= cx && sg.x1 - sg.x0 > w) { x0s.push(sg.x0); x1s.push(sg.x1) }
    }
    if (x0s.length >= 3) {
      x0s.sort((a, b) => a - b); x1s.sort((a, b) => a - b)
      const m0 = x0s[x0s.length >> 1]
      const m1 = x1s[x1s.length >> 1]
      if (m0 <= bl.x0 + B && m1 >= bl.x1 - B) return [m0 - 0.5 * B, m1 + 0.5 * B]
    }
  }
  return [bl.x0 - B, bl.x1 + B]
}

function findCaptions(ctx: Ctx): FloatBlock[] {
  const { lay, B, page, segKind } = ctx
  const used = new Set<number>()
  const out: FloatBlock[] = []
  const rows = lay.rows
  for (let ri = 0; ri < rows.length; ri++) {
    for (const sg of rows[ri].segs) {
      if (used.has(sg.id) || segKind[sg.id] !== null) continue
      const head = captionHead(sg.text)
      if (!head) continue
      if (head.strength === 'word' && !labelOwnSpan(page, sg, head.labelEnd)) continue
      // A caption starts its line: no text of the same line just before it (a justified gap).
      const before = rows[ri].segs.find(o => o !== sg && o.x1 <= sg.x0 + 0.5 && sg.x0 - o.x1 < 1.2 * B && isProseSeg(o))
      if (before) continue
      // ...and starts its block: the line above, in the same column, is not the paragraph it continues.
      let above: Seg | null = null
      for (let k = ri - 1; k >= 0 && !above; k--) {
        if (sg.y0 - rows[k].bottom > 2 * B) break
        for (const o of rows[k].segs) if (stacked(o, sg) && (!above || o.y1 > above.y1)) above = o
      }
      if (above && segKind[above.id] === null && isProseSeg(above) &&
          Math.abs(above.fs - sg.fs) <= CAPTION_FS_TOL * sg.fs && sg.y0 - above.y1 < CAPTION_FLOW_GAP * B) continue

      // The caption's lines: the next line of the same column, as close as caption lines are, in the
      // same size — for as long as the line before it was full (a caption is one paragraph).
      const segs = [sg]
      const x0 = sg.x0
      // Pieces of a caption line that a stretched gap split off: on the line, to its right, same size, and
      // `fits` (inside the width the caption is known to have — the next column's text is not). Returns the
      // right end of the whole line.
      const takePieces = (line: Seg, fits: (o: Seg) => boolean): number => {
        let end = line.x1
        for (const o of rows[line.row].segs) {
          if (o === line || segs.includes(o) || used.has(o.id) || segKind[o.id] !== null) continue
          if (Math.abs(o.fs - line.fs) > CAPTION_FS_TOL * line.fs || captionHead(o.text)) continue
          if (o.x0 >= line.x1 && o.x0 - end <= 3 * B && fits(o)) { segs.push(o); end = Math.max(end, o.x1) }
        }
        return end
      }
      let last = sg
      let lastEnd = sg.x1
      let x1 = lastEnd
      // A hanging caption indents its lines to where the text after the label starts.
      const hang = sg.glyphs > 0 ? (sg.x1 - sg.x0) * Math.min(1, head.labelEnd / Math.max(1, squash(sg.text).length)) : 0
      for (let k = ri + 1; k < rows.length && segs.filter(o => o.row !== sg.row).length < CAPTION_MAX_LINES; k++) {
        const r = rows[k]
        if (r.top - last.y1 > CAPTION_LINE_GAP * B) break
        // (A raised or lowered fragment — "−2", "i" — can sit on a row of its own: it is not the next line.)
        const next = r.segs.find(o => !used.has(o.id) && segKind[o.id] === null && stacked(o, last) &&
          !(o.glyphs < FOOTNOTE_MIN_LINE_GLYPHS && o.fs < 0.8 * last.fs))
        if (!next) continue // a line of another column (or of a float beside this one) in between
        const fsOk = Math.abs(next.fs - last.fs) <= CAPTION_FS_TOL * last.fs && Math.abs(next.fs - sg.fs) <= 2 * CAPTION_FS_TOL * sg.fs
        if (!fsOk || captionHead(next.text)) break
        // The first line's own pieces are known only now: the second line runs on under them.
        if (last === sg) {
          lastEnd = takePieces(sg, o => next.x0 <= sg.x1 && next.x1 >= (o.x0 + o.x1) / 2)
          x1 = lastEnd
        }
        // The next line of the same caption starts where the caption does (or under the text after its
        // label, or is centred under it) and is no wider: a wider line that starts elsewhere is the text
        // beside a narrow float, run together with the caption's line.
        const centred = Math.abs((next.x0 + next.x1) / 2 - (sg.x0 + sg.x1) / 2) <= B
        const flush = next.x0 >= x0 - CAPTION_FULL_LINE * B && next.x0 <= x0 + hang + CAPTION_FULL_LINE * B
        if (!(flush || centred) || (head.strength !== 'alone' && next.x1 > x1 + CAPTION_FULL_LINE * B)) break
        // "TABLE IV" / "Table 1" alone on its line: its title follows, whatever the first line's width.
        const full = (last === sg && head.strength === 'alone') || lastEnd >= x1 - CAPTION_FULL_LINE * B
        if (!full) break
        segs.push(next)
        last = next
        const known = x1
        lastEnd = takePieces(next, o => o.x1 <= known + CAPTION_FULL_LINE * B)
        if (lastEnd > x1) x1 = lastEnd
      }
      // A label alone is a caption only with its title under it ("TABLE IV" / "COMPARISON OF …").
      if (head.strength === 'alone' && last === sg) continue
      // The raised and lowered fragments inside the caption's box are its own.
      const top = Math.min(...segs.map(o => o.y0))
      const bottom = Math.max(...segs.map(o => o.y1))
      for (let k = ri; k < rows.length && rows[k].top <= bottom; k++) {
        for (const o of rows[k].segs) {
          if (segs.includes(o) || used.has(o.id) || segKind[o.id] !== null || o.glyphs >= FOOTNOTE_MIN_LINE_GLYPHS) continue
          if (o.x0 >= x0 - 0.5 * B && o.x1 <= x1 + 0.5 * B && o.y0 >= top - 0.5 * B && o.y1 <= bottom + 0.5 * B) segs.push(o)
        }
      }
      for (const s of segs) used.add(s.id)
      out.push({
        type: head.type, segs, top: Math.min(...segs.map(s => s.y0)), bottom: Math.max(...segs.map(s => s.y1)),
        x0: Math.min(x0, ...segs.map(s => s.x0)), x1,
      })
    }
  }
  return out
}

/** What stops a walk away from a caption: the running text, a heading, a numbered display, another
 *  caption, the page furniture. */
function stopsFloat(ctx: Ctx, segs: Seg[], captions: Set<number>): boolean {
  const { B, segKind } = ctx
  for (const sg of segs) {
    // The page furniture, another float's caption, a float already found.
    if (segKind[sg.id] !== null || captions.has(sg.id)) return true
    if (sg.fs >= BODY_FS_MIN * B && isProseSeg(sg)) return true
    if (sg.fs >= HEADING_MIN * B && sg.glyphs <= 100 && WORD_RUN.test(sg.text) && segs.length === 1) return true
    if (sg.fs >= BODY_FS_MIN * B && EQUATION_NUMBER.test(sg.text.trim())) return true
  }
  return false
}

/** Mathematics at the size of the text: a line of a display (as opposed to a row of cells). */
const MATH_SIGN = /[=≤≥≈≠∑∏∫∂∇→←⇒⇔∀∃∈∉⊂⊆∪∩±×·]/u

/** A row of a table: cells side by side, or a line of numbers. */
function tableLike(ctx: Ctx, segs: Seg[]): boolean {
  const { B } = ctx
  if (segs.length >= 3) return true
  const numeric = segs.some(sg => numericTokenRatio(sg.text) >= 0.5)
  const small = segs.every(sg => sg.fs < BODY_FS_MIN * B)
  if (segs.length === 2) {
    // Two pieces of an aligned display ("f(x) =" … "∑ …") are not two cells.
    if (!numeric && !small && segs.some(sg => MATH_SIGN.test(sg.text))) return false
    return true
  }
  const sg = segs[0]
  return !!sg && !isProseSeg(sg) && (numeric || small)
}

/**
 * Walk away from a caption (dir -1: up, +1: down) inside [x0, x1] and collect the float's own text.
 *   figure  everything up to the first white band wider than FLOAT_ROW_GAP (the picture), then only
 *           small type (labels above the picture)
 *   table   rows that follow each other closely, up to the first wider band
 */
function walkFloat(ctx: Ctx, bl: FloatBlock, x0: number, x1: number, dir: -1 | 1, mode: 'figure' | 'table', captions: Set<number>): Seg[] {
  const { lay, B } = ctx
  const rows = lay.rows
  const capRows = bl.segs.map(s => s.row)
  let k = dir < 0 ? Math.min(...capRows) - 1 : Math.max(...capRows) + 1
  let edge = dir < 0 ? bl.top : bl.bottom
  let pastBand = false
  const taken: Seg[] = []
  for (; k >= 0 && k < rows.length; k += dir) {
    const r = rows[k]
    const segs = segsWithin(r, x0, x1).filter(s => !bl.segs.includes(s))
    if (segs.length === 0) continue
    const gap = dir < 0 ? edge - r.bottom : r.top - edge
    // A table hangs close under (or over) its caption — a little further from it than its rows from each other.
    if (gap > (mode === 'table' && taken.length === 0 ? TABLE_CAPTION_GAP : FLOAT_ROW_GAP) * B) {
      if (mode === 'table') break
      pastBand = true
    }
    if (stopsFloat(ctx, segs, captions)) break
    // Above the picture only what is small and inside the float's width is still its own: a label, a legend.
    if (pastBand && !segs.every(s => s.fs < FIGURE_SMALL * B && s.x0 >= x0 - 0.5 * B && s.x1 <= x1 + 0.5 * B)) break
    // The first / last line of a paragraph (short, so not prose by itself) — the line beyond it is the text.
    const beyond = rows[k + dir]
    if (beyond && segs.some(s => s.fs >= BODY_FS_MIN * B && /[\p{L}]/u.test(s.text))) {
      const bsegs = segsWithin(beyond, x0, x1)
      const bgap = dir < 0 ? r.top - beyond.bottom : beyond.top - r.bottom
      if (bgap < CAPTION_FLOW_GAP * B && bsegs.some(s => s.fs >= BODY_FS_MIN * B && isProseSeg(s))) break
    }
    if (mode === 'table' && !tableLike(ctx, segs)) break
    taken.push(...segs)
    edge = dir < 0 ? r.top : r.bottom
  }
  return taken
}

/** The first rows of a side of the caption look like a table (cells side by side, numbers, small type). */
function tableSide(ctx: Ctx, bl: FloatBlock, x0: number, x1: number, dir: -1 | 1, captions: Set<number>): boolean {
  const { lay, B } = ctx
  const rows = lay.rows
  const capRows = bl.segs.map(s => s.row)
  let edge = dir < 0 ? bl.top : bl.bottom
  let seen = 0
  for (let k = dir < 0 ? Math.min(...capRows) - 1 : Math.max(...capRows) + 1; k >= 0 && k < rows.length && seen < 2; k += dir) {
    const segs = segsWithin(rows[k], x0, x1).filter(s => !bl.segs.includes(s))
    if (segs.length === 0) continue
    const gap = dir < 0 ? edge - rows[k].bottom : rows[k].top - edge
    if (gap > (seen === 0 ? TABLE_CAPTION_GAP : FLOAT_ROW_GAP) * B || stopsFloat(ctx, segs, captions) || !tableLike(ctx, segs)) return false
    seen++
    edge = dir < 0 ? rows[k].top : rows[k].bottom
  }
  return seen >= 2
}

/** Algorithm steps under an "Algorithm 1" caption: numbered lines and the keywords of pseudo-code. */
const ALGORITHM_LINE = /^(?:\d{1,3}\s*:|(?:input|output|require|ensure|parameters?|return|for|for each|foreach|while|if|else|end|repeat|until|procedure|function|initialize|do)\b)/i

function walkAlgorithm(ctx: Ctx, bl: FloatBlock, x0: number, x1: number, captions: Set<number>): Seg[] {
  const { lay, B, segKind } = ctx
  const rows = lay.rows
  let edge = bl.bottom
  const taken: Seg[] = []
  for (let k = Math.max(...bl.segs.map(s => s.row)) + 1; k < rows.length; k++) {
    const segs = segsWithin(rows[k], x0, x1)
    if (segs.length === 0) continue
    if (rows[k].top - edge > FLOAT_ROW_GAP * B) break
    if (segs.some(s => segKind[s.id] !== null || captions.has(s.id))) break
    const first = segs.reduce((a, b) => (b.x0 < a.x0 ? b : a))
    if (!ALGORITHM_LINE.test(first.text.trim()) && !(first.glyphs <= 3 && /^\d{1,3}:?$/.test(first.text.trim()))) break
    taken.push(...segs)
    edge = rows[k].bottom
  }
  return taken
}

/** The white band (em) between a caption and the nearest text above it in [x0, x1] — the picture of a
 *  figure whose picture holds no text. Above the topmost text of a page it reaches to where a page's text
 *  area starts at the earliest. */
function bandAbove(ctx: Ctx, bl: FloatBlock, x0: number, x1: number): number {
  const rows = ctx.lay.rows
  for (let k = Math.min(...bl.segs.map(s => s.row)) - 1; k >= 0; k--) {
    const segs = segsWithin(rows[k], x0, x1).filter(s => !bl.segs.includes(s))
    if (segs.length > 0) return (bl.top - rows[k].bottom) / ctx.B
  }
  return (bl.top - TEXT_AREA_TOP * ctx.page.height) / ctx.B
}

/** What follows a caption in [x0, x1] is set off from it — a white band — or is not running text. */
function setOffBelow(ctx: Ctx, bl: FloatBlock, x0: number, x1: number): boolean {
  const { lay, B } = ctx
  for (let k = Math.max(...bl.segs.map(s => s.row)) + 1; k < lay.rows.length; k++) {
    const segs = segsWithin(lay.rows[k], x0, x1).filter(s => !bl.segs.includes(s))
    if (segs.length === 0) continue
    if (lay.rows[k].top - bl.bottom >= CAPTION_SET_OFF * B) return true
    return !segs.some(s => s.fs >= BODY_FS_MIN * B && isProseSeg(s))
  }
  return true
}

function classifyFloats(ctx: Ctx): void {
  const { B } = ctx
  const blocks = findCaptions(ctx)
  if (blocks.length === 0) return
  // Every line of every caption: a walk from one float stops at another's caption.
  const captions = new Set<number>()
  for (const bl of blocks) for (const s of bl.segs) captions.add(s.id)
  blocks.forEach((bl, g) => {
    const [x0, x1] = floatExtent(ctx, bl)
    let content: Seg[] = []
    if (bl.type === 'figure') content = walkFloat(ctx, bl, x0, x1, -1, 'figure', captions)
    else if (bl.type === 'algorithm') content = walkAlgorithm(ctx, bl, x0, x1, captions)
    else {
      // A table's caption sits above it or below it, by the template: the side with rows of cells.
      for (const dir of [-1, 1] as const) {
        if (tableSide(ctx, bl, x0, x1, dir, captions)) content.push(...walkFloat(ctx, bl, x0, x1, dir, 'table', captions))
      }
    }
    // A caption set in the size of the text has to show its float — the labels or rows found next to it,
    // or (a figure) the picture's white band above it — or at least look like a caption rather than a
    // paragraph: a few lines, set off from what follows. A paragraph that begins "Figure 1. At the top
    // level" at the head of a column (the sentence runs on from the foot of the one before) is neither.
    const fs = bl.segs.reduce((a, s) => a + s.fs * s.glyphs, 0) / Math.max(1, bl.segs.reduce((a, s) => a + s.glyphs, 0))
    if (fs >= CAPTION_TEXT_SIZE * B && content.length === 0 &&
        !(bl.type === 'figure' && bandAbove(ctx, bl, x0, x1) >= PICTURE_MIN_BAND) &&
        !(new Set(bl.segs.map(s => s.row)).size <= CAPTION_SHORT_LINES && setOffBelow(ctx, bl, x0, x1))) return
    for (const sg of [...bl.segs, ...content]) {
      if (ctx.segKind[sg.id] !== null) continue
      ctx.segKind[sg.id] = 'float'
      ctx.segFloat[sg.id] = g
    }
  })
}

/** Char-weighted median font size of what is on the page: the body size of last resort. */
function medianFontSize(lay: Layout): number | null {
  const items = lay.segs.map(s => ({ fs: s.fs, w: s.glyphs })).sort((a, b) => a.fs - b.fs)
  const total = items.reduce((a, b) => a + b.w, 0)
  if (total === 0) return null
  let acc = 0
  for (const it of items) { acc += it.w; if (acc >= total / 2) return it.fs }
  return null
}

/**
 * Which spans of a page are furniture rather than text flow.
 *
 * Returns one entry per `page.spans[i]`: null for text flow, otherwise
 *   'pageNumber'  a line that is only a page number, at the head or the foot of the page
 *   'header'      a running header line (the label above the text block)
 *   'footer'      a running footer line
 *   'footnote'    a block of notes below all the text flow of its column
 *   'margin'      rotated text in the page margin (the arXiv stamp, side watermarks)
 *
 * See the header of this file for the rules; the short version is that a line is only
 * flagged when its position, its size relative to the body, its isolation and the shape
 * of its text all agree. Never throws, never mutates its input.
 */
export function classifyPageFurniture(page: FurniturePage, opts: ClassifyOptions = {}): Array<FurnitureKind | null> {
  return classifyPageLayout(page, opts).kinds
}

/** What `classifyPageLayout` says about a page. */
export interface PageLayoutReading {
  /** One entry per span, as `classifyPageFurniture` returns it. */
  kinds: Array<FurnitureKind | null>
  /** For every span of kind 'float', which float of the page it belongs to (its caption and its text share
   *  one number); -1 for every other span. */
  floatGroup: Int32Array
}

/** `classifyPageFurniture`, plus which float each 'float' span belongs to. */
export function classifyPageLayout(page: FurniturePage, opts: ClassifyOptions = {}): PageLayoutReading {
  const spans = page.spans
  const out: Array<FurnitureKind | null> = new Array(spans.length).fill(null)
  const floatGroup = new Int32Array(spans.length).fill(-1)
  const result: PageLayoutReading = { kinds: out, floatGroup }
  if (spans.length === 0 || !(page.width > 0) || !(page.height > 0)) return result

  // A page typeset sideways (landscape table rotated into a portrait page) has no
  // horizontal text flow to measure against.
  let rotated = 0
  let upright = 0
  for (const s of spans) { const n = glyphCount(s.text); if (isHorizontal(s)) upright += n; else rotated += n }
  if (rotated > upright) return result

  classifyMargin(page, out)

  const lay = buildLayout(page)
  if (lay.segs.length === 0) return result
  const B = opts.bodyFontSize && opts.bodyFontSize > 0
    ? opts.bodyFontSize
    : (estimateBodyFontSize([page]) ?? medianFontSize(lay))
  if (!B) return result

  const sigs: EdgeSig[] = []
  if (opts.neighbours) for (const nb of opts.neighbours) if (nb.height > 0) sigs.push(...edgeSignatures(nb))

  const slide = page.width > page.height && B >= SLIDE_MIN_FONT
  const ctx: Ctx = { page, lay, B, slide, segKind: new Array(lay.segs.length).fill(null), segFloat: new Int32Array(lay.segs.length).fill(-1), sigs }
  classifyEdge(ctx, true)
  classifyEdge(ctx, false)
  // Slides have a foot line, not footnotes, and their figures are the page itself.
  if (!slide) {
    classifyFootnotes(ctx)
    classifyFloats(ctx)
  }

  for (let i = 0; i < spans.length; i++) {
    const si = lay.segOf[i]
    if (si >= 0 && out[i] === null) {
      out[i] = ctx.segKind[si]
      if (out[i] === 'float') floatGroup[i] = ctx.segFloat[si]
    }
  }
  return result
}

// ═══════════════════════════════════════════════════════════════════════════════
// DOM helper
// ═══════════════════════════════════════════════════════════════════════════════

/** Parse a CSS length / angle custom property such as "10.00px" or "-90deg". */
function cssNumber(v: string): number {
  const n = parseFloat(v)
  return Number.isFinite(n) ? n : NaN
}

/**
 * Read a rendered pdf.js text layer into a `FurniturePage`.
 *
 *   textLayerEl  the `.textLayer` element
 *   pageEl       the page element the layer sits in (its box is the origin)
 *   scale        the viewer's CSS px per PDF unit (so a 612-wide page at scale 1.5 is 918 px)
 *
 * `elements[i]` is the <span> of `page.spans[i]`. Skipped: <br>, `.endOfContent` and
 * `.markedContent` wrappers (they are not <span>s of text) and empty spans.
 *
 * Rects come from `getBoundingClientRect()`, so they are exactly what the selection is
 * painted from. The font size is read from pdf.js's own `--font-height` custom property
 * (the PDF size, independent of zoom and of the browser's minimum font size), falling
 * back to the computed `font-size`. A rotated span's rect is its axis-aligned box; its
 * `angle` comes from `--rotate`, falling back to the computed transform.
 *
 * Cost: one layout pass plus one rect read per span (~1–3 ms for a 1 000-span page).
 * Call it at mouse-up, not per frame. The page must be laid out (not display:none).
 */
export function spansFromTextLayer(
  textLayerEl: HTMLElement,
  pageEl: HTMLElement,
  scale: number,
): SpanReading {
  const k = scale > 0 ? scale : 1
  const pr = pageEl.getBoundingClientRect()
  const spans: FurnitureSpan[] = []
  const elements: HTMLElement[] = []
  const eolAfter: boolean[] = []
  let minFont = NaN
  const nodes = textLayerEl.querySelectorAll<HTMLElement>('span')
  for (let n = 0; n < nodes.length; n++) {
    const el = nodes[n]
    if (el.classList.contains('markedContent') || el.classList.contains('endOfContent')) continue
    const text = el.textContent ?? ''
    if (text === '') continue
    const r = el.getBoundingClientRect()

    let fontSize = cssNumber(el.style.getPropertyValue('--font-height'))
    if (!(fontSize > 0)) {
      // The computed size is the zoomed one times pdf.js's `--min-font-size` (1 unless the
      // browser enforces a minimum text size).
      if (Number.isNaN(minFont)) minFont = cssNumber(getComputedStyle(textLayerEl).getPropertyValue('--min-font-size')) || 1
      fontSize = parseFloat(getComputedStyle(el).fontSize) / (k * minFont)
    }

    let angle = cssNumber(el.style.getPropertyValue('--rotate'))
    if (Number.isNaN(angle)) {
      const m = /^matrix\(([^)]+)\)$/.exec(getComputedStyle(el).transform)
      if (m) {
        const v = m[1].split(',').map(Number)
        angle = Math.atan2(v[1], v[0]) * 180 / Math.PI
      } else angle = 0
    }

    const sp: FurnitureSpan = { text, x: (r.left - pr.left) / k, y: (r.top - pr.top) / k, w: r.width / k, h: r.height / k, fontSize }
    if (Math.abs(angle) > 0.01) sp.angle = angle
    // pdf.js writes the PDF font's family as an inline generic ('monospace', 'sans-serif', ...).
    if (el.style.fontFamily.indexOf('monospace') >= 0) sp.mono = true
    spans.push(sp)
    elements.push(el)
    // pdf.js follows a span that ends a line with a <br>; that is where `Selection.toString()` breaks the line.
    let next = el.nextSibling
    while (next && next.nodeType === 3 && !(next.nodeValue ?? '').trim()) next = next.nextSibling
    eolAfter.push(next !== null && next.nodeName === 'BR')
  }
  return { page: { width: pr.width / k, height: pr.height / k, spans }, elements, eolAfter }
}

/** The part of a pdf.js `TextContent` that `furniturePageFromTextContent` reads. */
export interface TextContentLike {
  items: ReadonlyArray<{ str?: string; transform?: number[]; width?: number; height?: number; fontName?: string }>
  styles: Record<string, { fontFamily?: string; vertical?: boolean } | undefined>
}

/** Share of a font's size that a text-layer span sits above its baseline when the browser's own
 *  measure is not at hand (pdf.js falls back to the same 0.8). The DOM reading differs by ~0.03 em. */
const ASCENT_RATIO = 0.8

/**
 * The same `FurniturePage` that `spansFromTextLayer` reads off a rendered text layer, computed from
 * pdf.js's `page.getTextContent()` and `page.view` instead — with no rendering, no DOM and ~0.2 ms
 * per page. This is how a viewer gets the neighbours of a page (and a document-wide body size) for
 * pages it has not rendered. It follows pdf.js's `TextLayer.#appendText` line by line, at scale 1;
 * against a real text layer the x, w, h and font sizes agree to 0.05 px and y to ~0.4 px (the browser's
 * ascent for the span's font versus the 0.8 assumed here). A page with a /Rotate gives null (read its
 * rendered text layer instead).
 */
export function furniturePageFromTextContent(
  content: TextContentLike,
  view: readonly number[],
  rotation = 0,
): FurniturePage | null {
  if (rotation % 360 !== 0 || view.length < 4) return null
  const [x0, , , y1] = view
  const width = view[2] - view[0]
  const height = view[3] - view[1]
  const spans: FurnitureSpan[] = []
  for (const it of content.items) {
    const str = it.str
    const t = it.transform
    if (str === undefined || str === '' || !t || t.length < 6) continue
    const style = (it.fontName !== undefined ? content.styles[it.fontName] : undefined) ?? {}
    // Util.transform([1, 0, 0, -1, -x0, y1], t)
    const a = t[0], b = -t[1], c = t[2], d = -t[3]
    const e = t[4] - x0, f = y1 - t[5]
    let angle = Math.atan2(b, a)
    if (style.vertical) angle += Math.PI / 2
    const fh = Math.hypot(c, d)
    const asc = fh * ASCENT_RATIO
    const w = (style.vertical ? it.height : it.width) ?? 0
    let x: number, y: number, ww = w, hh = fh
    let deg = 0
    if (angle === 0) {
      x = e; y = f - asc
    } else {
      const cos = Math.cos(angle), sin = Math.sin(angle)
      const left = e + asc * sin
      const top = f - asc * cos
      const xs: number[] = []
      const ys: number[] = []
      for (const [u, v] of [[0, 0], [w, 0], [0, fh], [w, fh]]) { xs.push(left + u * cos - v * sin); ys.push(top + u * sin + v * cos) }
      x = Math.min(...xs); y = Math.min(...ys)
      ww = Math.max(...xs) - x; hh = Math.max(...ys) - y
      deg = angle * 180 / Math.PI
      deg = ((deg % 360) + 540) % 360 - 180
    }
    const sp: FurnitureSpan = { text: str, x, y, w: ww, h: hh, fontSize: fh }
    if (Math.abs(deg) > 0.01) sp.angle = deg
    if (style.fontFamily === 'monospace') sp.mono = true
    spans.push(sp)
  }
  return { width, height, spans }
}

/**
 * Classify with the geometry pdf.js computed, not the geometry the browser painted.
 *
 * `spansFromTextLayer` reads rects off the rendered text layer, and a browser measures a span's
 * width with its own font metrics: WebKit's come out up to 5 % wider than Chrome's, which closes the
 * gutter between two columns and made the footnote classifier miss BERT's footnotes in the real
 * WKWebView. `furniturePageFromTextContent` follows pdf.js's own arithmetic instead and gives the
 * same numbers in every engine, and it is what the classifier was tuned on.
 *
 * Both readings list the same spans in the same order (both skip empty strings and keep blank ones),
 * so when `content` has exactly as many spans as `dom` and every text is identical, `content`'s
 * geometry (x, y, w, h, fontSize, angle, mono) stands in for `dom`'s and the indices still line up with
 * `SpanReading.elements`. Anything else (no content reading, a /Rotate page, a text layer that has
 * not finished or that was cut) gives `dom` back unchanged: wrong geometry is worse than browser geometry.
 */
export function preferContentGeometry(dom: FurniturePage, content: FurniturePage | null): FurniturePage {
  if (!content || content.spans.length !== dom.spans.length) return dom
  for (let i = 0; i < dom.spans.length; i++) {
    if (dom.spans[i].text !== content.spans[i].text) return dom
  }
  return content
}

/**
 * For every span element of a page, the part of `range` that covers it, by element index (the
 * index into `SpanReading.elements`, which is DOM order). Elements the range does not cover
 * (or only touches) are absent. The first and last entries are the spans the selection starts
 * and ends in. Same walk as the viewer's `collectSelectionRectsByPage`, but per span.
 */
export function selectedTextBySpan(range: Range, elements: readonly HTMLElement[]): Map<number, string> {
  const out = new Map<number, string>()
  for (let i = 0; i < elements.length; i++) {
    const el = elements[i]
    if (!range.intersectsNode(el)) continue
    let text = ''
    for (let c = el.firstChild; c; c = c.nextSibling) {
      if (c.nodeType !== 3 || !range.intersectsNode(c)) continue
      const value = c.nodeValue ?? ''
      const a = c === range.startContainer ? range.startOffset : 0
      const b = c === range.endContainer ? range.endOffset : value.length
      if (b > a) text += value.slice(a, b)
    }
    if (text !== '') out.set(i, text)
  }
  return out
}

/**
 * The text of a selection without the spans in `dropped`: the selected text of every kept span in
 * DOM order, a line break after each span that ends a line (as `Selection.toString()` puts one),
 * not trimmed. Join the pages with "\n" and trim, as the viewer does with `sel.toString()`.
 */
export function keptSelectionText(
  selected: ReadonlyMap<number, string>,
  eolAfter: readonly boolean[],
  dropped: ReadonlySet<number>,
): string {
  let out = ''
  const order = [...selected.keys()].sort((a, b) => a - b)
  for (const i of order) {
    if (dropped.has(i)) continue
    out += selected.get(i)
    if (eolAfter[i]) out += '\n'
  }
  return out
}

/** One selected page's contribution to `keptSelectionTextAcrossPages`. */
export interface KeptTextPage {
  /** `selectedTextBySpan(range, reading.elements)`. */
  selected: ReadonlyMap<number, string>
  /** `SpanReading.eolAfter` of that page. */
  eolAfter: readonly boolean[]
  /** `planFurnitureDrop(...)[i]`; absent = nothing dropped. */
  dropped?: ReadonlySet<number>
}

/**
 * The text of the whole highlight: the selected text of every kept span, in DOM order across the
 * pages, with ONE "\n" wherever the PDF breaks a line — after a span that ends a line, at a page
 * boundary, and where a dropped span stood in between. That last rule is the point: the page number
 * between "…composition." and "In contrast" is `"…composition.\n2\nIn contrast"` in `Selection.toString()`;
 * with `keptSelectionText` per page joined by "\n" it would become `"…composition.\n\nIn contrast"`
 * (a blank line in the middle of a sentence), here it is `"…composition.\nIn contrast"`.
 * With nothing dropped the result is `Selection.toString().trim()` up to line breaks, and better at the
 * one place they differ: a page boundary. The browser writes a line break there only when it crosses
 * selectable boxes; where the pages' own boxes are `user-select: none` and only the text layers opt
 * back in (Argus's app shell, App.vue), WebKit runs the last line of one page into the first line of
 * the next — "…considered composition.\n2In contrast" — and this rebuild keeps the line break.
 * Trimmed, like the string it replaces.
 */
export function keptSelectionTextAcrossPages(pages: readonly KeptTextPage[]): string {
  let out = ''
  let owed = false // a line break is due before the next text
  pages.forEach((pg, p) => {
    if (p > 0) owed = true
    const order = [...pg.selected.keys()].sort((a, b) => a - b)
    for (const i of order) {
      if (pg.dropped?.has(i)) {
        if (pg.eolAfter[i]) owed = true
        continue
      }
      const t = pg.selected.get(i) ?? ''
      if (t === '') continue
      if (owed && out !== '' && !out.endsWith('\n')) out += '\n'
      owed = false
      out += t
      if (pg.eolAfter[i]) owed = true
    }
  })
  return out.trim()
}

// ═══════════════════════════════════════════════════════════════════════════════
// The cross-page policy
// ═══════════════════════════════════════════════════════════════════════════════

export interface SelectedFurniturePage {
  page: FurniturePage
  /** `classifyPageFurniture(page, ...)` of this page. */
  kinds: ReadonlyArray<FurnitureKind | null>
  /** The selection also covers the page before this one / the page after it. */
  fromPrev: boolean
  toNext: boolean
  /** Index (into `page.spans`) of the span the selection STARTS in — on the first
   *  selected page — and ENDS in — on the last one. Omit when it lies in no span. */
  startIndex?: number | null
  endIndex?: number | null
  /** `classifyPageLayout(page, ...).floatGroup`: which float each 'float' span belongs to. Without it every
   *  float of the page counts as one. */
  floatGroup?: ArrayLike<number>
}

/** Which side of its page a flagged span is on: the head (top), the tail (bottom) or neither. */
function sideOf(page: FurniturePage, i: number, kind: FurnitureKind): 'head' | 'tail' | 'margin' | 'float' {
  if (kind === 'header') return 'head'
  if (kind === 'footer' || kind === 'footnote') return 'tail'
  if (kind === 'margin') return 'margin'
  if (kind === 'float') return 'float'
  const s = page.spans[i]
  return s.y + s.h / 2 < page.height / 2 ? 'head' : 'tail'
}

/**
 * The policy: for every selected page, the indices of the spans to leave out of the
 * highlight (its rects and its text).
 *
 * Nothing is dropped unless the selection spans at least two pages. On a page that
 * continues onto the next one its tail furniture goes; on a page that continues from the
 * previous one its head furniture goes; margin stamps go from every page of the
 * selection. A side stays untouched if the user's own start (first page) or end (last
 * page) point lies inside furniture of that side.
 *
 * One more case, because the text layer lists spans in the PDF's content-stream order and
 * not top to bottom: a paper whose foot page number is the FIRST item of the page (EMNLP / ACL
 * style: the number of page N+1 is its first text item, then the running head, then the body)
 * sweeps that number into a selection that ends in the body of the page, and one whose
 * running head is the LAST item sweeps it into a selection that starts in the body. Such a
 * span is on the wrong side of the user's own end point — on the last page a tail span that
 * comes before the span the user ended in, on the first page a head span that comes after the
 * span the user started in — so it cannot be part of what they read, and it goes as well. The
 * same guard holds: it only applies when the user's own start / end span is not furniture of
 * that side, and never to a single-page selection.
 */
export function furnitureToDrop(pages: readonly SelectedFurniturePage[]): Array<Set<number>> {
  const out: Array<Set<number>> = pages.map(() => new Set<number>())
  if (!pages.some(p => p.fromPrev || p.toNext)) return out
  pages.forEach((p, pi) => {
    // Sides (and floats) the user pointed at on purpose.
    const keep = new Set<string>()
    const keepFloats = new Set<number>()
    for (const idx of [p.startIndex, p.endIndex]) {
      if (idx == null) continue
      const kind = p.kinds[idx]
      if (kind === 'float') keepFloats.add(p.floatGroup ? p.floatGroup[idx] : -1)
      else if (kind) keep.add(sideOf(p.page, idx, kind))
    }
    // Where the user's own end (last page) / start (first page) lies, for furniture the text layer
    // lists on the far side of it.
    const end = p.fromPrev && !p.toNext && p.endIndex != null ? p.endIndex : null
    const start = p.toNext && !p.fromPrev && p.startIndex != null ? p.startIndex : null
    p.kinds.forEach((kind, i) => {
      if (!kind) return
      // A figure or a table lies across the text wherever it is set: every one the selection runs through
      // goes, but the one the selection starts or ends in.
      if (kind === 'float') {
        if (!keepFloats.has(p.floatGroup ? p.floatGroup[i] : -1)) out[pi].add(i)
        return
      }
      const side = sideOf(p.page, i, kind)
      if (keep.has(side)) return
      if (
        side === 'margin'
        || (side === 'tail' && (p.toNext || (end !== null && i < end)))
        || (side === 'head' && (p.fromPrev || (start !== null && i > start)))
      ) out[pi].add(i)
    })
  })
  return out
}

/** One page of a selection, as the viewer sees it. */
export interface SelectionPage {
  page: FurniturePage
  /** The selected text by span index (`selectedTextBySpan`); spans not in the map are not selected. */
  selected: ReadonlyMap<number, string>
}

/**
 * The whole job in one call: given the consecutive pages a selection covers (in page order, each
 * with the spans it selects), the indices of the spans to leave out of every page.
 *
 * It classifies each page — the other selected pages (and `opts.neighbours`, any further pages of
 * the document whose text layers are at hand) serve as the neighbours that vouch for a running
 * head / page number — takes the body size from all of them, derives `fromPrev` / `toNext` from
 * the page order and the first / last selected span from `selected`, and applies `furnitureToDrop`.
 *
 * Returns empty sets (nothing dropped) when fewer than two pages are given, and also when the policy
 * would leave NO selected text at all (the user selected furniture on purpose).
 */
export function planFurnitureDrop(
  pages: readonly SelectionPage[],
  opts: { bodyFontSize?: number; neighbours?: readonly FurniturePage[] } = {},
): Array<Set<number>> {
  return planFurnitureDropReport(pages, opts).drop
}

/** What `planFurnitureDropReport` says about the spans it leaves out — for a "skipped 2 page numbers and a
 *  footnote" notice and an "include them" toggle (which is just the same rebuild with `drop` empty). */
export interface FurnitureDropReport {
  /** The same sets `planFurnitureDrop` returns: per selected page, the span indices to leave out. */
  drop: Array<Set<number>>
  /** The left-out spans that carry text, in reading order. `page` is the index into the `pages` argument,
   *  `index` the span index on that page, `text` the span's own text (not just its selected part). */
  items: Array<{ page: number; index: number; kind: FurnitureKind; text: string }>
  /** How many left-out spans there are of each kind (blank ones not counted). */
  counts: Partial<Record<FurnitureKind, number>>
}

/** `planFurnitureDrop`, plus what was left out and why. */
export function planFurnitureDropReport(
  pages: readonly SelectionPage[],
  opts: { bodyFontSize?: number; neighbours?: readonly FurniturePage[] } = {},
): FurnitureDropReport {
  const none = (): FurnitureDropReport => ({ drop: pages.map(() => new Set<number>()), items: [], counts: {} })
  if (pages.length < 2) return none()
  const all = [...pages.map(p => p.page), ...(opts.neighbours ?? [])]
  const B = opts.bodyFontSize ?? estimateBodyFontSize(all) ?? undefined
  const items: SelectedFurniturePage[] = pages.map((p, i) => {
    const others = all.filter(q => q !== p.page).slice(0, MAX_NEIGHBOURS)
    const idx = [...p.selected.keys()].sort((a, b) => a - b)
    const layout = classifyPageLayout(p.page, { bodyFontSize: B, neighbours: others })
    return {
      page: p.page,
      kinds: layout.kinds,
      floatGroup: layout.floatGroup,
      fromPrev: i > 0,
      toNext: i < pages.length - 1,
      startIndex: i === 0 && idx.length > 0 ? idx[0] : null,
      endIndex: i === pages.length - 1 && idx.length > 0 ? idx[idx.length - 1] : null,
    }
  })
  // Only spans the selection really covers count: a dropped span outside it changes nothing in the
  // highlight, so it must neither be listed nor claimed in the notice ("skipped page numbers").
  const drop = furnitureToDrop(items).map((set, pi) => new Set([...set].filter(i => pages[pi].selected.has(i))))
  // Everything selected would go: leave the selection alone.
  let kept = 0
  pages.forEach((p, i) => { for (const [k, t] of p.selected) if (!drop[i].has(k) && t.trim() !== '') kept++ })
  if (kept === 0) return none()
  const report: FurnitureDropReport = { drop, items: [], counts: {} }
  pages.forEach((p, pi) => {
    for (const idx of [...drop[pi]].sort((a, b) => a - b)) {
      const text = p.page.spans[idx]?.text ?? ''
      const kind = items[pi].kinds[idx]
      if (!kind || text.trim() === '' || (p.selected.get(idx) ?? '').trim() === '') continue
      report.items.push({ page: pi, index: idx, kind, text: text.trim() })
      report.counts[kind] = (report.counts[kind] ?? 0) + 1
    }
  })
  // Only blank spans (or none) would go: nothing visible changes, so there is nothing to report.
  return report.items.length === 0 ? none() : report
}

/** At most this many neighbour pages are read per page: each costs one more layout pass. */
const MAX_NEIGHBOURS = 4

