// Structural section (chapter) detection for an open PDF.
//
// Two layers, tried in order (the AI fallback lives in Rust and is manual-only):
//   1. Embedded outline  — the PDF's own bookmarks (getOutline). Most reliable
//      when present, but most academic PDFs (arXiv/bioRxiv) ship without one.
//   2. Heading heuristic — per-page text with font-size analysis + heading
//      regexes. Font size is the primary discriminator; regex classifies level.
//
// Both produce the same shape as the backend `PaperSections` so either can be
// persisted via `save_sections`.

import type { PDFDocumentProxy } from 'pdfjs-dist'
import type { PaperSection, PaperSections } from '../types'

// Minimum sections for a heuristic result to be considered trustworthy — a
// standard paper has at least Intro / Method / Results / Conclusion / References.
const MIN_HEURISTIC_SECTIONS = 3

// Common top-level section names in English papers (matched at line start).
const KEYWORD_RE =
  /^(abstract|introduction|related\s+work|background|preliminaries|motivation|methods?|methodology|materials\s+and\s+methods|approach|experimental\s+setup|experiments?|evaluation|results?(?:\s+and\s+discussion)?|analysis|discussion|conclusions?|future\s+work|limitations|acknowledge?ments?|references|bibliography|appendix|appendices)\b/i

// Numbered headings: "1 Introduction", "2. Methods", "3.1 Setup", "A.1 …".
const NUMBERED_RE = /^(\d+(?:\.\d+){0,3})[.)]?\s+[A-Za-z]/

function normalize(s: string): string {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()
}

// ── Layer 1: embedded outline ────────────────────────────────────────────────

/** Resolve a pdf.js destination (named or explicit) to a 1-based page number. */
async function destToPage(doc: PDFDocumentProxy, dest: unknown): Promise<number> {
  try {
    let explicit = dest
    if (typeof dest === 'string') {
      explicit = await doc.getDestination(dest)
    }
    if (!Array.isArray(explicit) || explicit.length === 0) return 0
    const ref = explicit[0]
    if (ref == null || typeof ref !== 'object') return 0
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const pageIndex = await doc.getPageIndex(ref as any)
    return pageIndex + 1
  } catch {
    return 0
  }
}

/* eslint-disable @typescript-eslint/no-explicit-any */
async function walkOutline(
  doc: PDFDocumentProxy,
  items: any[],
  level: number,
  out: PaperSection[],
): Promise<void> {
  for (const item of items) {
    const title = (item.title ?? '').trim()
    if (title) {
      out.push({ title, level, page: await destToPage(doc, item.dest) })
    }
    if (Array.isArray(item.items) && item.items.length) {
      await walkOutline(doc, item.items, level + 1, out)
    }
  }
}
/* eslint-enable @typescript-eslint/no-explicit-any */

async function extractOutline(doc: PDFDocumentProxy): Promise<PaperSection[] | null> {
  let outline: unknown
  try {
    outline = await doc.getOutline()
  } catch {
    return null
  }
  if (!Array.isArray(outline) || outline.length === 0) return null
  const out: PaperSection[] = []
  await walkOutline(doc, outline, 1, out)
  return out.length ? out : null
}

// ── Layer 2: heading heuristic ───────────────────────────────────────────────

interface Line {
  text: string
  size: number
  page: number
  words: number
}

/** Group a page's text items into visual lines and record each line's max font size. */
/* eslint-disable @typescript-eslint/no-explicit-any */
function itemsToLines(items: any[], page: number): Line[] {
  // Bucket by rounded baseline-y (transform[5]); items on the same line share it.
  const byRow = new Map<number, { parts: { x: number; str: string }[]; size: number }>()
  for (const it of items) {
    const str: string = it.str ?? ''
    if (!str) continue
    const tr = it.transform ?? [1, 0, 0, 1, 0, 0]
    const y = Math.round(tr[5])
    const x = tr[4] ?? 0
    const size = Number(it.height) || Math.hypot(tr[2] ?? 0, tr[3] ?? 0) || 0
    const row = byRow.get(y) ?? { parts: [], size: 0 }
    row.parts.push({ x, str })
    row.size = Math.max(row.size, size)
    byRow.set(y, row)
  }
  const lines: Line[] = []
  for (const row of byRow.values()) {
    const text = row.parts
      .sort((a, b) => a.x - b.x)
      .map(p => p.str)
      .join('')
      .replace(/\s+/g, ' ')
      .trim()
    if (!text) continue
    lines.push({ text, size: row.size, page, words: text.split(' ').length })
  }
  return lines
}
/* eslint-enable @typescript-eslint/no-explicit-any */

/** Most common rounded font size across the document — the body-text size. */
function bodySize(lines: Line[]): number {
  const freq = new Map<number, number>()
  for (const l of lines) {
    const k = Math.round(l.size * 2) / 2
    freq.set(k, (freq.get(k) ?? 0) + l.words)
  }
  let best = 0
  let bestCount = -1
  for (const [k, c] of freq) {
    if (c > bestCount) {
      best = k
      bestCount = c
    }
  }
  return best
}

function classify(text: string): { level: number; title: string } | null {
  const num = NUMBERED_RE.exec(text)
  if (num) {
    const level = Math.min(3, num[1].split('.').length)
    return { level, title: text }
  }
  if (KEYWORD_RE.test(text)) {
    return { level: 1, title: text }
  }
  return null
}

/* eslint-disable @typescript-eslint/no-explicit-any */
async function extractHeuristic(doc: PDFDocumentProxy): Promise<PaperSection[] | null> {
  const allLines: Line[] = []
  for (let i = 1; i <= doc.numPages; i++) {
    const page = await doc.getPage(i)
    const tc = await page.getTextContent()
    allLines.push(...itemsToLines(tc.items as any[], i))
    page.cleanup()
  }
  if (allLines.length === 0) return null

  const body = bodySize(allLines)

  // Running headers/footers repeat the same text on many pages — ignore them.
  const pageFreq = new Map<string, Set<number>>()
  for (const l of allLines) {
    const key = normalize(l.text)
    if (!key) continue
    ;(pageFreq.get(key) ?? pageFreq.set(key, new Set()).get(key)!).add(l.page)
  }

  const out: PaperSection[] = []
  let lastKey = ''
  for (const l of allLines) {
    if (l.words > 12 || l.text.length > 90) continue
    const key = normalize(l.text)
    if (!key || key === lastKey) continue
    if ((pageFreq.get(key)?.size ?? 0) >= 3) continue // running header

    const big = body > 0 && l.size >= body * 1.06
    const cls = classify(l.text)
    if (!cls) continue
    // Font size is the primary signal; allow exact keyword lines at body size too.
    const exactKeyword = cls.level === 1 && l.words <= 4
    if (!big && !exactKeyword) continue

    out.push({ title: cls.title, level: cls.level, page: l.page })
    lastKey = key
  }

  return out.length >= MIN_HEURISTIC_SECTIONS ? out : null
}
/* eslint-enable @typescript-eslint/no-explicit-any */

// ── Orchestration ────────────────────────────────────────────────────────────

/**
 * Compute a section index for an open PDF: embedded outline first, then the
 * heading heuristic. Returns null when neither yields anything — the caller
 * then leaves the AI fallback (manual) as the only remaining option.
 */
export async function computeSections(doc: PDFDocumentProxy): Promise<PaperSections | null> {
  const outline = await extractOutline(doc)
  if (outline && outline.length) {
    return { source: 'outline', generated_at: new Date().toISOString(), sections: outline }
  }
  const heuristic = await extractHeuristic(doc)
  if (heuristic && heuristic.length) {
    return { source: 'heuristic', generated_at: new Date().toISOString(), sections: heuristic }
  }
  return null
}

/**
 * Run structural detection for a paper by its slug, loading the PDF ourselves —
 * used by the manual "re-detect" button so it works even when the paper isn't
 * currently open in a viewer. pdf.js is imported dynamically to keep it out of
 * the main bundle.
 */
export async function detectSectionsForSlug(slug: string): Promise<PaperSections | null> {
  const { invoke } = await import('@tauri-apps/api/core')
  const pdfjsLib = await import('pdfjs-dist')
  if (!pdfjsLib.GlobalWorkerOptions.workerSrc) {
    const workerUrl = (await import('pdfjs-dist/legacy/build/pdf.worker.min.mjs?url')).default
    pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl
  }
  const bytes = await invoke<number[]>('read_pdf_bytes', { slug })
  const doc = await pdfjsLib.getDocument({
    data: new Uint8Array(bytes),
    isOffscreenCanvasSupported: false,
  }).promise
  try {
    return await computeSections(doc)
  } finally {
    doc.destroy()
  }
}
