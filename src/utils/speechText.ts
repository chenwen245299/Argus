import { mergeWrappedLines } from './highlightText'

/**
 * Turning a PDF selection into something a voice should read, and cutting it into
 * pieces a speech endpoint will take.
 *
 * Pure on purpose (no Vue, no DOM, no Tauri): it is unit-tested under Node, and
 * both halves are rules that are easy to get subtly wrong and expensive to get
 * wrong — text-to-speech is billed per character, and a voice that reads
 * "Eslami et al. comma twenty eighteen" aloud is worse than no voice at all.
 *
 * The two entry points are `prepareSpeechText` (clean the selection) and
 * `chunkForSpeech` (split it). `speech.read()` runs one and then the other.
 */

// ── Preparing the text ────────────────────────────────────────────────────────

export interface PrepareOptions {
  /** Drop bracketed / parenthetical literature citations instead of reading them. */
  skipCitations: boolean
}

/** Typographic ligatures PDFs keep as one glyph; a voice mispronounces them. */
const LIGATURES: Record<string, string> = {
  'ﬀ': 'ff', 'ﬁ': 'fi', 'ﬂ': 'fl', 'ﬃ': 'ffi', 'ﬄ': 'ffl',
  'ﬅ': 'st', 'ﬆ': 'st',
}

/**
 * The marker left where a citation was cut out. NUL cannot occur in the input —
 * `prepareSpeechText` strips control characters first — so it can never be
 * mistaken for text, which a private-use character could (PDF symbol fonts use
 * those).
 */
const GAP = '\u0000'

/**
 * Selection text -> one clean paragraph a voice can read.
 *
 * Line breaks go first (`mergeWrappedLines` is the same rule the highlight
 * sidebar uses, so hyphenation and CJK joins behave identically), then the
 * odd characters PDFs carry, then — only when asked — the citations.
 */
export function prepareSpeechText(raw: string, opts: PrepareOptions): string {
  if (!raw) return ''
  let text = mergeWrappedLines(raw)
  text = text
    // Control characters (NUL included, see GAP), then the invisible ones.
    .replace(/[\u0000-\u0008\u000e-\u001f\u007f]/g, '')
    .replace(/[\u00ad\u200b\u2060\ufeff]/g, '')
    .replace(/[ﬀ-ﬆ]/g, (c) => LIGATURES[c] ?? c)
    .replace(/\s+/g, ' ')
    .trim()
  if (!text) return ''
  if (opts.skipCitations) text = stripCitations(text)
  return text.replace(/\s+/g, ' ').trim()
}

// -- Citation grammar ----------------------------------------------------------
//
// The grammar is deliberately narrow. A bracket is removed only when its WHOLE
// content parses as a citation list, so `[CLS]`, `[MASK]`, `(a)`, `(1)`,
// `(Figure 2)`, `(see Table 3)` and `(i.e. the data)` are never touched, and
// anything unusual is simply read out. Wrongly dropping a word of the paper is
// worse than reading a stray "[Smith 19]".

const NUM = String.raw`[1-9]\d{0,2}`
const NUM_ITEM = String.raw`${NUM}(?:\s*[–—-]\s*${NUM})?`
/** `[12]`, `[1, 2]`, `[3-5]`, `[1,2,5-7]` — the digits between the brackets. */
const NUMERIC_BODY = new RegExp(
  String.raw`^\s*${NUM_ITEM}(?:\s*[,，;；]\s*${NUM_ITEM})*\s*$`,
)

// 1600-2029: a citation year, not "2048" (a size) — the PDFs this reads cite
// Bayes (1763) and Gauss (1809) as readily as last year's preprint.
const YEAR = String.raw`(?:1[6-9]\d\d|20[0-2]\d)[a-z]?`
/** `2018`, `2018a`, `1998a,b`, `2016, 2018`. */
const YEARS = String.raw`${YEAR}(?:\s*[,，]\s*(?:${YEAR}|[a-z](?!\p{L})))*`
/** A capitalised name word (hyphens, apostrophes, diacritics, a trailing initial dot) or a particle. */
const WORD = String.raw`(?:\p{Lu}[\p{L}\p{M}'’\-]*\.?|(?:van|von|de|der|den|di|da|del|della|la|le|du|dos|das|zu|ten|ter)(?=\s))`
const PERSON = String.raw`${WORD}(?:\s+${WORD}){0,3}`
/** `Smith`, `Du and Mordatch`, `Hinton, LeCun, and Bengio`, `Xie et al.`, `van der Maaten and Hinton`. */
const AUTHORS = String.raw`${PERSON}(?:(?:\s*[,，]\s*(?:and\s+|&\s*)?|\s+and\s+|\s*&\s*)${PERSON})*(?:\s+et\s+al\.?)?`
const ENTRY = String.raw`${AUTHORS}(?:\s*[,，]\s*|\s+)${YEARS}`
/** "see", "see also", "e.g.,", "cf." ... — only a lead-in to a citation, never the whole of one. */
const LEAD_WORD = String.raw`(?:[Ss]ee(?:\s+also)?|[Aa]lso|[Ee]\.g\.|[Cc]f\.|[Ii]\.e\.|[Ff]or\s+(?:example|instance)|[Rr]eviewed\s+in|[Cc]ompare)`
/** One or more, so "see, e.g., Hirschberg and Manning, 2015" lands too. */
const LEAD_IN = String.raw`(?:${LEAD_WORD}[,;]?\s+)+`
const LEAD_IN_AT_START = new RegExp(`^${LEAD_IN}`, 'u')
const LOCATOR = String.raw`pp?\.\s*\d+(?:\s*[–-]\s*\d+)?`
/** `Eslami et al., 2018`, `Hinton, 2002; LeCun et al., 2006`, `Xie et al., 2016, Du and Mordatch, 2019`. */
const AUTHOR_YEAR_BODY = new RegExp(
  String.raw`^(?:${LEAD_IN})?${ENTRY}(?:\s*[,;，；]\s*${ENTRY})*(?:\s*[,，]\s*${LOCATOR})?$`,
  'u',
)

/**
 * Capitalised words that open a parenthetical date aside, not a citation: "(In 2019)".
 * "A" and "An" are left out on purpose — "An" is a surname ("An et al., 2019").
 */
const NOT_A_NAME = new Set([
  'in', 'since', 'until', 'till', 'before', 'after', 'from', 'by', 'circa', 'early', 'late',
  'mid', 'around', 'during', 'as', 'of', 'for', 'the', 'on', 'at', 'between',
])
/**
 * A label straight before a number that happens to look like a year: "(Epoch 1500)",
 * "(Iteration 2000)", "(Version 2020)". A citation puts a comma between author and
 * year or says "et al."; these never do, so they are only rejected when the label
 * is directly followed by the number.
 */
const LABEL_THEN_YEAR = new RegExp(
  String.raw`(?:^|[,;，；]\s*)(?:epoch|iter|iteration|step|version|ver|release|round|trial|run|seed|episode|frame|batch|fold|checkpoint|generation|cycle|update|sample|size|dim|width|height|length|resolution|level|stage|phase|case|task|class|id|no|number)s?\.?\s+\d`,
  'iu',
)
/** "(March 2020)" and "(Summer 2019)" are dates in the prose, not references. */
const DATE_WORDS = new Set([
  'january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september',
  'october', 'november', 'december', 'jan', 'feb', 'mar', 'apr', 'jun', 'jul', 'aug', 'sep',
  'sept', 'oct', 'nov', 'dec', 'spring', 'summer', 'fall', 'autumn', 'winter',
])

/** Words that, right before `[1, 2]`, say it is an interval rather than two references. */
const INTERVAL_WORDS = new Set([
  'interval', 'intervals', 'range', 'ranges', 'domain', 'between', 'over', 'within', 'ranging',
  'scale', 'normalized', 'normalised', 'clipped', 'clamped', 'bounded', 'lies', 'belongs',
])

/**
 * "a scale of [1, 10]", "clip to [1, 255]", "sampled from [2, 12]", "defined on [1, 5]":
 * a preposition straight before two integers. Applies to the two-integer form only,
 * and not to `in` — "as shown in [1, 5]" is a citation far more often than an interval.
 */
const INTERVAL_PREPS = new Set(['of', 'to', 'from', 'on', 'at'])

/**
 * ...unless the phrase in front of the preposition is one of the ways prose points at
 * literature: "based on [1, 2]", "compared to [3, 4]", "adapted from [5, 6]", "the work
 * of [1, 2]". Dropping a citation costs a spoken "one, two"; dropping the paper's own
 * words costs content, so the two lists err towards keeping.
 */
const CITING_HEADS = new Set([
  'based', 'compared', 'relative', 'similar', 'similarly', 'close', 'due', 'according', 'thanks', 'owing',
  'refer', 'refers', 'referred', 'referring', 'adapted', 'taken', 'borrowed', 'derived', 'built', 'inspired',
  'work', 'works', 'study', 'studies', 'paper', 'papers', 'author', 'authors', 'result', 'results',
  'extension', 'extensions', 'variant', 'variants', 'version', 'versions', 'follow', 'follows', 'following',
  'introduced', 'proposed', 'described', 'presented', 'see', 'cf', 'compare', 'analogous', 'akin', 'prior',
])

const OPEN_TO_CLOSE: Record<string, string> = {
  '[': ']', '［': '］', '【': '】', '(': ')', '（': '）',
}
const SQUARE_OPENERS = new Set(['[', '［', '【'])

/** A bracket pair with no bracket nested inside; contents capped so a stray "(" cannot swallow a page. */
const BRACKET_RE = /([\[［【(（])([^\[\]［］【】()（）]{1,240})([\]］】)）])/g

function lastWord(before: string): string {
  const m = /([\p{L}\p{N}_]+)[\s=]*$/u.exec(before.slice(-48))
  return m ? m[1] : ''
}

/** The word before the last word: "based" in "based on [1, 2]". */
function wordBeforeLast(before: string): string {
  const m = /([\p{L}\p{N}_]+)[\s=]+[\p{L}\p{N}_]+[\s=]*$/u.exec(before.slice(-64))
  return m ? m[1] : ''
}

/**
 * `foo(` — a call, not prose: no space between an identifier and the parenthesis.
 * `foo_bar`, `os.argv`: a name with an underscore or a member access. Whatever
 * follows such a token with `[2]` is an index (`torch.Size([2, 6])`,
 * `layer_sizes[1]`), never a reference.
 */
const CODE_BEFORE = /[\p{L}\p{N}_]\($|_[\p{L}\p{N}]*$|[\p{L}\p{N}_]\.[\p{L}_][\p{L}\p{N}_]*$/u

function isNumericCitation(
  inner: string,
  before: string,
  attached: boolean,
  after: string,
  followsIndex: boolean,
): boolean {
  if (!NUMERIC_BODY.test(inner)) return false
  const items = inner.split(/[,，;；]/).map((s) => s.trim()).filter(Boolean)
  // A range must climb: `[5-3]` is not a citation range.
  for (const it of items) {
    const r = /^(\d+)\s*[–—-]\s*(\d+)$/.exec(it)
    if (r && Number(r[2]) <= Number(r[1])) return false
  }
  // After a relation or an assignment it is a list or an interval in the maths or
  // the code, however many numbers it holds: `x = [1, 2, 3]`, `t ∈ [0, 1]`.
  const prev = before.replace(/\s+$/, '')
  const lastChar = prev.slice(-1)
  if (lastChar !== '' && '∈=≤≥<>∪∩⊂×'.includes(lastChar)) return false
  // `[4]` glued to a following lower-case word is part of a name: `calix[4]arene`,
  // `cucurbit[8]uril`, and `[3,3]-sigmatropic` with the hyphen in between. A
  // reference mark is followed by a space or punctuation.
  if (/^-?[a-z]/.test(after)) return false
  // `M[2][3]`, `A[0] [1]`: the second bracket is another index. Only after a bracket
  // that was itself left alone — `[1][2]` after two citations is still one hole.
  if (followsIndex) return false
  if (attached) {
    // `x[1]`, `W[2]`: indexing a variable, not a reference. Only a very short token
    // can be one — `Bengio[3]` and `GAN[5]` are references with the space lost.
    const token = /[\p{L}\p{N}_]+$/u.exec(before)?.[0] ?? ''
    // A variable is a Latin or Greek letter or two. The trailing run after the last
    // CJK character is all that can be one: "文献[3]" and "图[7]" are Chinese words
    // with a reference mark, "设x[1]" is a variable that follows a Chinese verb.
    let at = token.length
    while (at > 0 && !CJK_CHAR.test(token[at - 1])) at--
    const name = token.slice(at)
    if (name.length > 0 && name.length <= 2 && /\p{L}/u.test(name)) return false
    if (CODE_BEFORE.test(before)) return false
  }
  // `[1, 2]` / `[3-5]` after "range", "interval" ... is an interval.
  const twoIntegers = items.length === 2 && items.every((s) => /^\d+$/.test(s))
  const intervalLike = twoIntegers || (items.length === 1 && /[–—-]/.test(items[0]))
  if (intervalLike) {
    const word = lastWord(before).toLowerCase()
    if (INTERVAL_WORDS.has(word)) return false
    if (twoIntegers) {
      const head = wordBeforeLast(before).toLowerCase()
      // "a scale of [1, 10]", "clip to [1, 255]" — but not "based on [1, 2]".
      if (INTERVAL_PREPS.has(word) && !CITING_HEADS.has(head)) return false
      // "k in [1, 5]": a one-letter variable before `in` is maths, never prose.
      if (word === 'in' && /^\p{L}$/u.test(head)) return false
    }
  }
  return true
}

function isAuthorYearCitation(inner: string): boolean {
  const body = inner.trim()
  if (!AUTHOR_YEAR_BODY.test(body)) return false
  const words = body.match(/\p{Lu}[\p{L}\p{M}'’\-]*/gu) ?? []
  const lower = words.map((w) => w.toLowerCase().replace(/\.$/, ''))
  if (lower.length === 0) return true // lowercase-particle names only; cannot be a date
  if (LABEL_THEN_YEAR.test(body)) return false
  if (lower.every((w) => DATE_WORDS.has(w))) return false
  // "(CVPR 2019)", "(WMT 2014)": a venue or a benchmark edition, not an author. A
  // person's name has a lower-case letter in it ("Hinton"); initials ("G. E.") are
  // too short to count as an acronym.
  const isAcronym = (w: string) => w.replace(/[.'’\-]/g, '').length >= 2 && !/\p{Ll}/u.test(w)
  if (words.every(isAcronym)) return false
  const first = /^[\p{L}'’\-]+/u.exec(body.replace(LEAD_IN_AT_START, ''))
  if (first && NOT_A_NAME.has(first[0].toLowerCase())) return false
  return true
}

/** Remove citations, leaving a GAP marker, then close the hole they leave. */
function stripCitations(text: string): string {
  let removed = false
  // Where the last numeric bracket that was left alone ended (see `followsIndex`).
  let lastIndexEnd = -1
  const marked = text.replace(
    BRACKET_RE,
    (whole: string, open: string, inner: string, close: string, offset: number, all: string) => {
      if (OPEN_TO_CLOSE[open] !== close) return whole
      // Only the tail is ever inspected; slicing the whole prefix per bracket is quadratic.
      const before = all.slice(Math.max(0, offset - 64), offset)
      const attached = offset > 0 && !/\s/.test(all[offset - 1])
      const after = all.slice(offset + whole.length, offset + whole.length + 2)
      const followsIndex = lastIndexEnd >= 0 && /^\s?$/.test(all.slice(lastIndexEnd, offset))
      const hit = SQUARE_OPENERS.has(open) && isNumericCitation(inner, before, attached, after, followsIndex)
        ? true
        : isAuthorYearCitation(inner)
      if (!hit) {
        if (SQUARE_OPENERS.has(open) && /^[\d\s,，;；–—-]+$/.test(inner)) lastIndexEnd = offset + whole.length
        return whole
      }
      removed = true
      return GAP
    },
  )
  return removed ? closeGaps(marked) : text
}

const CJK_CHAR = /[⺀-⿟　-〿぀-ヿ㄀-ㄯ㐀-䶿一-鿿豈-﫿＀-￯]/
const CLOSING_PUNCT = /^[.,;:!?)\]。，；：！？）］】]/

/**
 * Close the holes: "methods [x]." -> "methods.", "A [1], [2] and B" -> "A and B",
 * "(see [1])" -> nothing, and no stranded comma before the full stop.
 *
 * Done per gap rather than with a global "tidy punctuation" pass, so original
 * text that merely looks like leftovers ("etc.,", "...", " , " in a formula) is
 * never touched.
 */
function closeGaps(text: string): string {
  let s = text
  // Neighbouring citations (and the commas / dashes between them) are one hole.
  s = s.replace(/\u0000(?:[\s,;，；–—-]*\u0000)+/g, GAP)
  // A bracket that held nothing but a lead-in and a citation goes with it.
  const wrapped = new RegExp(String.raw`[(（\[［]\s*(?:${LEAD_IN})?\u0000\s*[)）\]］]`, 'g')
  s = s.replace(wrapped, GAP).replace(wrapped, GAP)
  s = s.replace(/\u0000(?:[\s,;，；–—-]*\u0000)+/g, GAP)

  const parts = s.split(GAP)
  let out = parts[0]
  for (let i = 1; i < parts.length; i++) out = joinAcrossGap(out, parts[i])
  return out
}

/** `s` without the trailing run of any of `chars`. Char by char: a `/x+$/` regex rescans the whole string. */
function trimTail(s: string, chars: string): string {
  let e = s.length
  while (e > 0 && chars.includes(s[e - 1])) e--
  return e === s.length ? s : s.slice(0, e)
}

function joinAcrossGap(left: string, right: string): string {
  const l = trimTail(left, ' ')
  const r = right.charCodeAt(0) === 0x20 ? right.replace(/^ +/, '') : right
  if (!l) return r
  if (!r) return trimTail(l, ',;:，；：')
  if (CLOSING_PUNCT.test(r)) {
    // Closing punctuation hugs the word before the hole; a comma that was
    // introducing the citation goes with it ("methods, [1]." -> "methods.").
    return trimTail(l, ',;:，；：') + r
  }
  if ('(（[［【'.includes(l[l.length - 1])) return l + r
  if (CJK_CHAR.test(l[l.length - 1]) || CJK_CHAR.test(r[0])) return l + r
  return l + ' ' + r
}

// ── Chunking ──────────────────────────────────────────────────────────────────

export interface ChunkOptions {
  /** Longest chunk the speech model accepts, counted in characters (code points). */
  maxChars: number
  /** Budget of the FIRST chunk. Short on purpose, so audio starts quickly. Default 200. */
  firstChars?: number
  /** Budget of every later chunk before the model's own limit. Default 900. */
  laterChars?: number
}

export const DEFAULT_FIRST_CHARS = 200
export const DEFAULT_LATER_CHARS = 900
const FALLBACK_MAX_CHARS = 500

/** Length in characters as a speech API counts them (code points, not UTF-16 units). */
export function charCount(s: string): number {
  let n = 0
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i)
    if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      const d = s.charCodeAt(i + 1)
      if (d >= 0xdc00 && d <= 0xdfff) i++
    }
    n++
  }
  return n
}

/** Words whose full stop is not a sentence end, whatever follows it. */
const ABBREVIATIONS = new Set([
  'al', 'vs', 'cf', 'fig', 'figs', 'eq', 'eqs', 'dr', 'mr', 'mrs', 'ms', 'prof', 'sr', 'jr',
  'st', 'vol', 'pp', 'ph', 'ca', 'viz', 'resp', 'approx', 'ref', 'refs', 'sec', 'secs', 'ch',
  'chap', 'tab', 'alg', 'algs', 'thm', 'prop', 'lem', 'cor', 'def', 'rem', 'inc', 'ltd', 'co',
  'corp', 'univ', 'dept', 'jan', 'feb', 'mar', 'apr', 'jun', 'jul', 'aug', 'sep', 'sept', 'oct',
  'nov', 'dec',
])

const CJK_TERMINATORS = '。．！？；‼⁇⁈⁉'
const CLOSERS = '"\'”’)]」』）］】》〉'

/** Whether the full stop at `s[i]` ends a sentence. `end` is the index after the run of `.!?`. */
function dotEndsSentence(s: string, i: number, end: number): boolean {
  if (end - i > 1) return true // "..." or "?!" — judged by what follows, below
  // The word just before the stop, split on dots so "e.g." / "U.S." / "Ph.D." end on one letter.
  const word = /([\p{L}\p{N}]+)$/u.exec(s.slice(Math.max(0, i - 24), i))?.[1] ?? ''
  if (word.length === 1 && /\p{L}/u.test(word)) return false // initial, "e.g.", "i.e."
  const low = word.toLowerCase()
  if (ABBREVIATIONS.has(low)) return false
  // "No. 5" is a number; "No. The" would be an answer. Digits decide.
  if (low === 'no' || low === 'nos') return !/^\s*\d/.test(s.slice(end))
  return true
}

/**
 * Cut one line into sentences. Latin `.` `!` `?` `;` need a following space (so
 * decimals and "3.14" stay whole) and — for a full stop — a following capital,
 * digit or opening quote (so "et al. showed" and "i.e. the" do not split); CJK
 * terminators need nothing. Closing quotes and brackets stay with their sentence.
 */
function splitLine(line: string): string[] {
  const out: string[] = []
  let start = 0
  const n = line.length
  let i = 0
  while (i < n) {
    const ch = line[i]
    if (CJK_TERMINATORS.includes(ch)) {
      let j = i + 1
      while (j < n && (CJK_TERMINATORS.includes(line[j]) || CLOSERS.includes(line[j]))) j++
      out.push(line.slice(start, j))
      start = i = j
      continue
    }
    if (ch === '.' || ch === '!' || ch === '?' || ch === ';') {
      let end = i + 1
      if (ch !== ';') while (end < n && (line[end] === '.' || line[end] === '!' || line[end] === '?')) end++
      let j = end
      while (j < n && CLOSERS.includes(line[j])) j++
      const atEnd = j >= n
      const spaced = !atEnd && line[j] === ' '
      if (atEnd || spaced) {
        let ok = true
        if (ch === '.') {
          ok = dotEndsSentence(line, i, end)
          if (ok && spaced) {
            // A lower-case continuation means the stop was not a sentence end.
            const next = /^ +(\S)/.exec(line.slice(j))?.[1] ?? ''
            if (next && /\p{Ll}/u.test(next) && end - i === 1) ok = false
          }
        }
        if (ok) {
          out.push(line.slice(start, j))
          start = j
        }
      }
      i = Math.max(j, i + 1)
      continue
    }
    i++
  }
  if (start < n) out.push(line.slice(start))
  return out.map((s) => s.trim()).filter(Boolean)
}

/** Text -> sentences, by the rules above. The chunker's unit, and what the reader lights up while it is spoken. */
export function splitSentences(text: string): string[] {
  const out: string[] = []
  for (const line of text.split(/[\r\n\u2028\u2029]+/)) {
    const flat = line.replace(/[ \t\f\v ]+/g, ' ').trim()
    if (flat) out.push(...splitLine(flat))
  }
  return out
}

const CLAUSE_LATIN = ',;:—'
const CLAUSE_CJK = '，、；：—'

function isCombining(cp: string): boolean {
  if (/\p{M}/u.test(cp) || cp === '\u200d') return true
  const c = cp.codePointAt(0) ?? 0
  return c >= 0x1f3fb && c <= 0x1f3ff // emoji skin-tone modifiers
}

/** Move a hard cut off the middle of a grapheme (combining mark, ZWJ sequence). */
function safeCut(cps: string[], k: number): number {
  let cut = k
  while (cut > 1 && (isCombining(cps[cut]) || cps[cut - 1] === '\u200d')) cut--
  return cut > 0 && cut < k + 1 ? cut : k
}

/** A space that sits inside an abbreviation ("et al.", "Fig. 3") must not be a cut. */
function isProtectedSpace(cps: string[], k: number): boolean {
  const before = cps.slice(Math.max(0, k - 12), k).join('')
  const after = cps.slice(k + 1, k + 4).join('')
  const prev = /([\p{L}]+)\.?$/u.exec(before)
  if (!prev) return false
  const w = prev[1].toLowerCase()
  if (w === 'et' && /^al/.test(after)) return true
  if (before.endsWith('.') && ABBREVIATIONS.has(w)) return true
  return false
}

/**
 * Cut `piece` (longer than `limit`) at the best boundary within `limit`
 * characters. A clause break beats a space beats a hard cut; the hard cut is
 * the last resort, for text with no punctuation or spaces at all.
 */
function splitLong(piece: string, limit: number): [string, string] {
  // Only the first `limit` characters can hold the cut, so look at a window of
  // them rather than spreading the whole piece into code points on every call —
  // that is quadratic for a long run with no sentence break. Twice the limit in
  // UTF-16 units always covers more than `limit` code points.
  const window = piece.length > limit * 2 + 2 ? piece.slice(0, limit * 2 + 2) : piece
  const cps = Array.from(window)
  if (window === piece && cps.length <= limit) return [piece, '']
  const lo = Math.max(1, Math.floor(limit * 0.4))
  const clauseAt = (k: number): boolean => {
    const prev = cps[k - 1]
    if (CLAUSE_CJK.includes(prev)) return true
    return CLAUSE_LATIN.includes(prev) && (k >= cps.length || cps[k] === ' ')
  }
  const spaceAt = (k: number): boolean => cps[k] === ' ' && !isProtectedSpace(cps, k)
  let cut = -1
  for (let k = limit; k >= lo && cut < 0; k--) if (clauseAt(k)) cut = k
  for (let k = limit; k >= lo && cut < 0; k--) if (spaceAt(k)) cut = k
  for (let k = lo - 1; k >= 1 && cut < 0; k--) if (clauseAt(k) || spaceAt(k)) cut = k
  if (cut < 0) cut = safeCut(cps, limit)
  const consumed = cps.slice(0, cut).join('')
  return [consumed.trimEnd(), piece.slice(consumed.length).trimStart()]
}

function joinSep(left: string, right: string): string {
  return CJK_CHAR.test(left.slice(-1)) || CJK_CHAR.test(right[0]) ? '' : ' '
}

/**
 * Split prepared text into chunks a speech endpoint accepts.
 *
 * - Sentence-aware (see `splitLine`); a sentence is only cut when it is longer
 *   than the budget on its own, first at a clause break, then at a space (never
 *   inside "et al." / "Fig. 3"), and only as a last resort mid-word.
 * - The FIRST chunk is short (`firstChars`, default 200) so audio starts after
 *   one small request; later chunks fill up to `min(maxChars, 900)`.
 * - No chunk ever exceeds `maxChars`, counted in characters (code points).
 * - Nothing is dropped or reordered: the non-space characters of the chunks, in
 *   order, are exactly those of the input.
 */
export function chunkForSpeech(text: string, opts: ChunkOptions): string[] {
  const maxChars = Number.isFinite(opts.maxChars) && opts.maxChars >= 1
    ? Math.floor(opts.maxChars)
    : FALLBACK_MAX_CHARS
  const clamp = (v: number | undefined, dflt: number): number =>
    Math.min(maxChars, Math.max(1, Math.floor(Number.isFinite(v as number) ? (v as number) : dflt)))
  const first = clamp(opts.firstChars, DEFAULT_FIRST_CHARS)
  const later = clamp(opts.laterChars, DEFAULT_LATER_CHARS)

  const queue = splitSentences(text).reverse() // pop() = next piece
  const chunks: string[] = []
  let cur = ''
  let curLen = 0
  const flush = () => {
    if (cur) chunks.push(cur)
    cur = ''
    curLen = 0
  }

  while (queue.length) {
    const piece = queue.pop() as string
    const plen = charCount(piece)
    const budget = chunks.length === 0 ? first : later
    const sep = cur ? joinSep(cur, piece) : ''
    if (curLen + sep.length + plen <= budget) {
      cur += sep + piece
      curLen += sep.length + plen
      continue
    }
    if (cur) {
      // Full: close it, and judge this piece again against the next chunk's budget.
      flush()
      queue.push(piece)
      continue
    }
    const [head, rest] = splitLong(piece, budget)
    if (head) chunks.push(head)
    if (rest) queue.push(rest)
  }
  flush()
  return chunks
}
