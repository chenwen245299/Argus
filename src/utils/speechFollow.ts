import { charCount, splitSentences } from './speechText'
import type { Rect } from '../types'

/**
 * Following a read on the page: which sentence is being spoken, where it sits in the
 * text it was read from, and how far the whole read has got.
 *
 * Pure (no DOM, no Vue), like the rest of the speech utilities: the viewer hands in
 * the selection as its text layer holds it, gets character ranges back and turns
 * those into rectangles itself.
 *
 * Both answers are estimates, deliberately simple ones:
 *
 * - WHERE a sentence is. What is spoken is the selection after `prepareSpeechText`
 *   (line breaks merged, ligatures spelled out, citations dropped), so it is not a
 *   substring of the page text. Its letters and digits are, in order, though —
 *   preparing only ever removes — so a sentence is found by matching a run of them.
 * - WHEN a sentence is spoken. The speech APIs return audio and no timings, so
 *   inside one clip the voice is taken to move through the characters at an even
 *   pace. On ordinary prose that is off by a fraction of a second.
 */

// ── Sentences of a read ───────────────────────────────────────────────────────

export interface SpokenSentence {
  /** The chunk (one speech request, one clip) it is read in. */
  chunk: number
  text: string
  /** Where it starts and ends inside its chunk's clip, as fractions of the clip, by characters. */
  from: number
  to: number
}

/** The sentences of a read, chunk by chunk, each with its share of its chunk's clip. */
export function spokenSentences(chunks: readonly string[]): SpokenSentence[] {
  const out: SpokenSentence[] = []
  chunks.forEach((chunk, ci) => {
    // A chunk is whole sentences joined back together (or one cut out of a sentence too
    // long for any chunk), so splitting it again yields the sentences it was built from.
    const parts = splitSentences(chunk)
    const total = parts.reduce((n, p) => n + charCount(p), 0)
    if (total === 0) return
    let acc = 0
    for (const text of parts) {
      const n = charCount(text)
      out.push({ chunk: ci, text, from: acc / total, to: (acc + n) / total })
      acc += n
    }
  })
  return out
}

/**
 * The sentence being spoken: the last one in chunk `chunk` whose share of the clip has
 * begun at `fraction` (0..1). -1 when the chunk has none.
 */
export function sentenceAt(sentences: readonly SpokenSentence[], chunk: number, fraction: number): number {
  let found = -1
  for (let i = 0; i < sentences.length; i++) {
    const s = sentences[i]
    if (s.chunk < chunk) continue
    if (s.chunk > chunk) break
    if (found < 0 || fraction >= s.from) found = i
  }
  return found
}

/**
 * How much of a read has been heard, 0..1: the chunks before `index` in full and
 * `fraction` of chunk `index`, each chunk weighted by its length — a chunk's clip lasts
 * roughly as long as its text, and the first chunk is kept short on purpose.
 */
export function readFraction(chunks: readonly string[], index: number, fraction: number): number {
  let total = 0
  let done = 0
  chunks.forEach((c, i) => {
    const n = charCount(c)
    total += n
    if (i < index) done += n
    else if (i === index) done += n * Math.min(1, Math.max(0, fraction))
  })
  return total > 0 ? Math.min(1, done / total) : 0
}

/** The inverse of `readFraction`: where `fraction` (0..1) of a read falls — a chunk, and how far into its clip. */
export function seekTarget(chunks: readonly string[], fraction: number): { index: number; fraction: number } | null {
  const lens = chunks.map((c) => charCount(c))
  const total = lens.reduce((a, b) => a + b, 0)
  if (total === 0) return null
  const goal = Math.min(1, Math.max(0, Number.isFinite(fraction) ? fraction : 0)) * total
  let acc = 0
  for (let i = 0; i < lens.length; i++) {
    if (lens[i] > 0 && (goal < acc + lens[i] || i === lens.length - 1)) {
      return { index: i, fraction: Math.min(1, (goal - acc) / lens[i]) }
    }
    acc += lens[i]
  }
  return null
}

// ── Finding the sentences in the page text ────────────────────────────────────

export interface TextRange {
  /** UTF-16 offsets into the source text; `end` is exclusive. */
  start: number
  end: number
}

const ALNUM = /[\p{L}\p{N}]/u

interface Keys {
  /** The letters and digits, case-folded, as one string. */
  keys: string
  /** Per UTF-16 unit of `keys`: where its character starts and ends in the text. */
  from: number[]
  to: number[]
}

/**
 * The letters and digits of `text`, case-folded, with where each one came from.
 * NFKC first, which spells ligatures (ﬁ) and full-width forms out the way
 * `prepareSpeechText` does, so both sides fold the same.
 */
function keysOf(text: string): Keys {
  let keys = ''
  const from: number[] = []
  const to: number[] = []
  for (let i = 0; i < text.length;) {
    const cp = text.codePointAt(i) ?? 0
    const len = cp > 0xffff ? 2 : 1
    for (const k of text.slice(i, i + len).normalize('NFKC').toLowerCase()) {
      if (!ALNUM.test(k)) continue
      keys += k
      for (let u = 0; u < k.length; u++) {
        from.push(i)
        to.push(i + len)
      }
    }
    i += len
  }
  return { keys, from, to }
}

/** Anchor lengths tried in turn: a long run is unambiguous, a short one survives a citation cut out of it. */
const ANCHORS = [24, 12, 6]
/**
 * How far past where a sentence should begin it may be found. What can sit in between
 * is what preparing dropped — a citation or two, a stray symbol — never a paragraph.
 */
const SLACK = 400

/** Where a run of `keys` (its head, or its tail with `tail`) occurs in `hay` between `from` and `limit`. */
function findAnchor(hay: string, keys: string, from: number, limit: number, tail = false): { at: number; len: number } | null {
  let tried = -1
  for (const n of ANCHORS) {
    const len = Math.min(n, keys.length)
    if (len === tried) continue
    tried = len
    const anchor = tail ? keys.slice(keys.length - len) : keys.slice(0, len)
    const at = hay.indexOf(anchor, from)
    if (at >= 0 && at <= limit) return { at, len }
  }
  return null
}

/**
 * Where a sentence (`keys`) begins in `hay`, at or after `cursor`. Its first letters
 * as a run, normally; when a citation was dropped right after its first word ("Then
 * (Smith et al., 2019) another…") that run is not on the page, so a run from further
 * in is found instead and the first letters are walked back to from there.
 */
function findStart(hay: string, keys: string, cursor: number): number | null {
  const direct = findAnchor(hay, keys, cursor, cursor + SLACK)
  if (direct) return direct.at
  for (let j = 6; j + 6 <= keys.length && j <= 72; j += 6) {
    const anchor = keys.slice(j, j + 12)
    const at = hay.indexOf(anchor, cursor + j)
    if (at < 0 || at > cursor + j + SLACK) continue
    // keys[0..j) in order, backwards from the anchor: the latest place they all fit.
    let p = at - 1
    let m = j - 1
    for (; m >= 0; m--, p--) {
      while (p >= cursor && hay[p] !== keys[m]) p--
      if (p < cursor) break
    }
    if (m < 0) return p + 1
  }
  return null
}

/**
 * Where each of `spoken` (sentences, in reading order) lies in `source`, the text it
 * was prepared from; null for one that cannot be found, which then simply is not
 * highlighted.
 *
 * A sentence runs up to where the next one begins, so the citation and the full stop
 * between two sentences go with the first — they belong to it on the page, even
 * though the voice skips the citation. The last sentence (or one whose successor was
 * not found) ends at the end of its own text, plus the punctuation stuck to it.
 */
export function locateSentences(source: string, spoken: readonly string[]): (TextRange | null)[] {
  const src = keysOf(source)
  const hay = src.keys
  const starts: (number | null)[] = []
  const lens: number[] = []

  // Starts, in order: each search begins where the previous sentence's letters end at
  // the earliest, since the spoken letters are a subsequence of the source's.
  let cursor = 0
  for (const s of spoken) {
    const keys = keysOf(s).keys
    lens.push(keys.length)
    const at = keys ? findStart(hay, keys, cursor) : null
    starts.push(at)
    if (at !== null) cursor = at + keys.length
  }

  const out: (TextRange | null)[] = []
  for (let i = 0; i < spoken.length; i++) {
    const at = starts[i]
    if (at === null) { out.push(null); continue }
    const start = src.from[at]
    const next = starts[i + 1]
    let end: number
    if (next !== null && next !== undefined && next > at) {
      end = src.from[next]
      while (end > start && /\s/.test(source[end - 1])) end--
    } else {
      const keys = keysOf(spoken[i]).keys
      const hit = findAnchor(hay, keys, at + Math.max(0, lens[i] - ANCHORS[0]), at + lens[i] + SLACK, true)
      const last = hit ? hit.at + hit.len - 1 : Math.min(hay.length, at + lens[i]) - 1
      end = src.to[last]
      // The full stop, closing bracket or quote that ends it — not the next citation, which
      // is separated by a space.
      while (end < source.length && /[\p{P}]/u.test(source[end]) && !/[([{［【（]/u.test(source[end])) end++
    }
    out.push(end > start ? { start, end } : null)
  }
  return out
}

// ── Geometry ──────────────────────────────────────────────────────────────────

/**
 * One stretch of text's rectangles, joined into one band per line. A line of a PDF text
 * layer is several spans whose boxes leave hairline gaps and overlaps between them,
 * which a tinted highlight shows as seams. Two boxes are on the same line when they
 * overlap vertically by at least half the smaller one, and joined when the horizontal
 * gap between them is under ~0.6 of a line — a word space, never a column gutter.
 */
export function lineBands(rects: readonly Rect[]): Rect[] {
  const sorted = rects
    .filter((r) => r.width > 0 && r.height > 0 && Number.isFinite(r.x) && Number.isFinite(r.y))
    .slice()
    .sort((a, b) => a.y - b.y || a.x - b.x)
  const bands: Rect[] = []
  for (const r of sorted) {
    const band = bands.find((b) => {
      const overlap = Math.min(b.y + b.height, r.y + r.height) - Math.max(b.y, r.y)
      const h = Math.min(b.height, r.height)
      const gap = Math.max(r.x - (b.x + b.width), b.x - (r.x + r.width))
      return overlap >= h * 0.5 && gap <= h * 0.6
    })
    if (!band) { bands.push({ ...r }); continue }
    const x1 = Math.min(band.x, r.x)
    const y1 = Math.min(band.y, r.y)
    const x2 = Math.max(band.x + band.width, r.x + r.width)
    const y2 = Math.max(band.y + band.height, r.y + r.height)
    band.x = x1
    band.y = y1
    band.width = x2 - x1
    band.height = y2 - y1
  }
  return bands
}
