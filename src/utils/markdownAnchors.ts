import type { Highlight } from '../types'

/** Visible DOM text only: KaTeX's hidden MathML and toolbar buttons are not evidence. */
export function markdownTextIndex(root: HTMLElement) {
  const nodes: { node: Text; start: number; end: number }[] = []
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: node => {
      const parent = node.parentElement
      if (parent?.closest('.katex-mathml,script,style,button,annotation,[data-markdown-overlay]')) return NodeFilter.FILTER_REJECT
      const marker = parent?.closest('[class*="vditor-ir__marker"]')
      // Vditor's code/math source uses marker--pre too. Keep its actual text
      // when expanded; exclude punctuation markers and the hidden source copy.
      if (marker && (!marker.classList.contains('vditor-ir__marker--pre')
        || (marker.isConnected && !Array.from(marker.getClientRects()).some(r => r.width && r.height)))) return NodeFilter.FILTER_REJECT
      const preview = parent?.closest('.vditor-ir__preview')
      if (preview?.isConnected && (!preview.getClientRects().length || getComputedStyle(preview).visibility === 'hidden')) return NodeFilter.FILTER_REJECT
      return NodeFilter.FILTER_ACCEPT
    },
  })
  let text = '', next: Node | null
  while ((next = walker.nextNode())) {
    const start = text.length
    text += next.textContent ?? ''
    nodes.push({ node: next as Text, start, end: text.length })
  }
  return { text, nodes }
}
export type MarkdownTextIndex = ReturnType<typeof markdownTextIndex>

function offsetAt(index: MarkdownTextIndex, container: Node, offset: number) {
  const caret = document.createRange()
  caret.setStart(container, offset); caret.collapse(true)
  for (const entry of index.nodes) {
    if (entry.node === container) return entry.start + offset
    if (caret.comparePoint(entry.node, entry.node.length) > 0) return entry.start
  }
  return index.text.length
}

export function markdownSelection(root: HTMLElement, range: Range) {
  if (!root.contains(range.startContainer) || !root.contains(range.endContainer)) return null
  const index = markdownTextIndex(root)
  let start = offsetAt(index, range.startContainer, range.startOffset)
  let end = offsetAt(index, range.endContainer, range.endOffset)
  const raw = index.text.slice(start, end)
  start += raw.length - raw.trimStart().length
  end -= raw.length - raw.trimEnd().length
  const text = index.text.slice(start, end)
  return text ? { text, start, end, prefix: index.text.slice(Math.max(0, start - 48), start), suffix: index.text.slice(end, end + 48) } : null
}

/** Edits may move a quote. Relocate only an exact, unambiguous quote/context match. */
export function locateMarkdownQuote(text: string, h: Pick<Highlight, 'text' | 'start_offset' | 'end_offset' | 'anchor_prefix' | 'anchor_suffix'>) {
  const quote = h.text
  if (!quote) return null
  const contextMatches = (start: number) => (!h.anchor_prefix || text.slice(Math.max(0, start - h.anchor_prefix.length), start) === h.anchor_prefix)
    && (!h.anchor_suffix || text.slice(start + quote.length, start + quote.length + h.anchor_suffix.length) === h.anchor_suffix)
  const start = h.start_offset
  if (start != null && h.end_offset === start + quote.length && text.slice(start, h.end_offset) === quote && contextMatches(start)) {
    return { start, end: start + quote.length }
  }
  const matches: number[] = [], contextual: number[] = []
  for (let at = text.indexOf(quote); at >= 0; at = text.indexOf(quote, at + 1)) {
    matches.push(at)
    if (contextMatches(at)) contextual.push(at)
  }
  const found = contextual.length === 1 ? contextual[0] : matches.length === 1 ? matches[0] : null
  return found == null ? null : { start: found, end: found + quote.length }
}

export function markdownRange(index: MarkdownTextIndex, start: number, end: number) {
  const first = index.nodes.find(n => n.end > start)
  const last = index.nodes.find(n => n.end >= end && n.start < end)
  if (!first || !last || start >= end) return null
  const range = document.createRange()
  range.setStart(first.node, start - first.start)
  range.setEnd(last.node, end - last.start)
  return range
}

/** Text-node ranges avoid painting enclosing inline/block boxes twice. */
export function markdownTextRects(index: MarkdownTextIndex, start: number, end: number) {
  const rects: DOMRect[] = []
  for (const entry of index.nodes) {
    if (entry.end <= start || entry.start >= end) continue
    const range = document.createRange()
    range.setStart(entry.node, Math.max(0, start - entry.start))
    range.setEnd(entry.node, Math.min(entry.node.length, end - entry.start))
    rects.push(...Array.from(range.getClientRects()))
  }
  return rects
}
