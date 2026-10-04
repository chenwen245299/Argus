import type { Highlight } from '../types'

/**
 * The text of a highlight as it should READ — one paragraph, not one fragment per
 * printed line.
 *
 * A PDF has no paragraphs, only positioned lines, so a selection comes back with a
 * line break at every wrap (`sel.toString()` of pdf.js's text layer). Left as is, an
 * exported Markdown quote becomes `> line` × N and the sidebar / copy / AI context
 * carry the wrap points, which cuts one sentence into shards.
 *
 * `Highlight.text` therefore stays EXACTLY as captured (it is the source of truth and
 * what "keep line breaks" shows), and every consumer that shows or exports a
 * highlight goes through `highlightDisplayText`. Nothing is rewritten on disk, so a
 * library full of old highlights is fixed the moment this ships, and an older build
 * (which strips the `keep_line_breaks` flag it does not know) merely merges again.
 *
 * The Rust twin is `src-tauri/src/highlight_text.rs` (export, MCP, embedding chunks).
 * The two must behave identically — keep every rule below in step with it.
 */

/** Characters that count as whitespace here. Spelled out rather than `\s` / `trim()`
 *  so the Rust twin can use the very same set (their built-ins differ at U+0085/U+FEFF). */
function isSpace(c: number): boolean {
  return (c >= 0x09 && c <= 0x0d) || c === 0x20 || c === 0xa0 || c === 0x1680
    || (c >= 0x2000 && c <= 0x200a) || c === 0x2028 || c === 0x2029 || c === 0x202f
    || c === 0x205f || c === 0x3000 || c === 0xfeff
}

/** Han, kana, bopomofo, CJK punctuation and full-width forms: scripts written with no
 *  spaces between words, so a wrap there must be rejoined without one. Hangul is left
 *  out on purpose — Korean separates words with spaces, and the wrap replaced one. */
function isCjk(c: number): boolean {
  return (c >= 0x2e80 && c <= 0x2fdf) || (c >= 0x3000 && c <= 0x303f)
    || (c >= 0x3040 && c <= 0x30ff) || (c >= 0x3100 && c <= 0x312f)
    || (c >= 0x3400 && c <= 0x4dbf) || (c >= 0x4e00 && c <= 0x9fff)
    || (c >= 0xf900 && c <= 0xfaff) || (c >= 0xff00 && c <= 0xffef)
    || (c >= 0x20000 && c <= 0x2fa1f)
}

/** Hyphen and dashes a line can end on: `-` U+2010 U+2011 U+2013 U+2014. */
function isDash(c: number): boolean {
  return c === 0x2d || c === 0x2010 || c === 0x2011 || c === 0x2013 || c === 0x2014
}

const SOFT_HYPHEN = 0xad

function lastCodePoint(s: string): number {
  const n = s.length
  const lo = s.charCodeAt(n - 1)
  if (n >= 2 && lo >= 0xdc00 && lo <= 0xdfff) {
    const hi = s.charCodeAt(n - 2)
    if (hi >= 0xd800 && hi <= 0xdbff) return (hi - 0xd800) * 0x400 + (lo - 0xdc00) + 0x10000
  }
  return lo
}

/** Split on `\r\n`, `\n`, `\r`, U+2028 and U+2029. */
function splitLines(text: string): string[] {
  return text.split(/\r\n|[\n\r\u2028\u2029]/)
}

function trimSpace(s: string): string {
  let a = 0
  let b = s.length
  while (a < b && isSpace(s.charCodeAt(a))) a++
  while (b > a && isSpace(s.charCodeAt(b - 1))) b--
  return s.slice(a, b)
}

/** Whether `text` has a line break worth a toggle (more than one non-blank line). */
export function hasLineBreaks(text: string): boolean {
  let seen = 0
  for (const line of splitLines(text)) {
    if (trimSpace(line)) seen++
    if (seen > 1) return true
  }
  return false
}

/**
 * Rejoin the lines of a wrapped selection into one paragraph.
 *
 * - Lines are trimmed and blank ones dropped: a PDF selection cannot tell a wrap from
 *   a paragraph break, and this is the "one highlight, one paragraph" view. (A list,
 *   code or an equation is what "keep line breaks" is for.)
 * - Between two lines: a single space — except with no space at all when either side
 *   of the break is CJK, or when the line ends in a soft hyphen (dropped), or in a
 *   hyphen / dash that hugs the word before it (`long-` + `term` -> `long-term`,
 *   `COVID-` + `19` -> `COVID-19`).
 * - The hyphen is KEPT: telling a wrapped word (`sen-`/`tence`) from a compound
 *   (`long-`/`term`) needs a dictionary, and deleting a real hyphen is worse than
 *   leaving a visible one. "Keep line breaks" still shows the original.
 */
export function mergeWrappedLines(text: string): string {
  let acc = ''
  for (const raw of splitLines(text)) {
    const line = trimSpace(raw)
    if (!line) continue
    if (!acc) { acc = line; continue }

    const last = lastCodePoint(acc)
    const first = line.codePointAt(0)!
    if (last === SOFT_HYPHEN) {
      acc = acc.slice(0, -1) + line
    } else if (isCjk(last) || isCjk(first)) {
      acc += line
    } else if (isDash(last) && acc.length >= 2 && !isSpace(acc.charCodeAt(acc.length - 2))) {
      acc += line
    } else {
      acc += ' ' + line
    }
  }
  return acc
}

/** The shape of a highlight record this file needs. */
type TextSource = Pick<Highlight, 'text' | 'keep_line_breaks' | 'start_offset' | 'end_offset'>

/**
 * What a highlight shows and exports: the merged paragraph, unless the user chose to
 * keep the original line breaks. Ebook records (they carry offsets) are returned
 * untouched — there a newline is a real paragraph boundary, not a wrap.
 */
export function highlightDisplayText(h: TextSource): string {
  if (h.start_offset != null || h.end_offset != null) return h.text
  return h.keep_line_breaks ? h.text : mergeWrappedLines(h.text)
}
