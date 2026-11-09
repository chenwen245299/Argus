import { Marked, type Token, type Tokens } from 'marked'
import { markdownSelection, markdownTextIndex } from './markdownAnchors'

// Use an independent parser: chat's global marked renderer adds copy buttons,
// syntax highlighting and diagrams that are not part of a document's text.
const parser = new Marked()
const cjk = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/gu
const word = /[\p{L}\p{N}]+(?:['’\-][\p{L}\p{N}]+)*/gu
const blockSelector = 'p,li,h1,h2,h3,h4,h5,h6,pre,tr,blockquote'
const Segmenter = (Intl as unknown as {
  Segmenter?: new (locale: string, options: { granularity: 'grapheme' }) => { segment(text: string): Iterable<{ segment: string }> }
}).Segmenter
const graphemes = Segmenter ? new Segmenter('en', { granularity: 'grapheme' }) : null

function htmlText(html: string): string {
  // Template contents remain inert: counting never loads images or runs HTML.
  const template = document.createElement('template')
  template.innerHTML = html
  template.content.querySelectorAll('script,style,svg,math,template,img,button,input,.katex-mathml').forEach(el => el.remove())
  template.content.querySelectorAll(`${blockSelector},div,br`).forEach(el => el.append('\n\n'))
  return template.content.textContent ?? ''
}

function decodeEntities(text: string) {
  return text.includes('&') ? htmlText(text.replace(/</g, '&lt;')) : text
}

function tokenText(token: Token): string {
  if (token.type === 'image' || token.type === 'def' || token.type === 'hr' || token.type === 'checkbox') return ''
  if (token.type === 'space' || token.type === 'br') return '\n'
  if (token.type === 'html') return htmlText(token.text)
  if (token.type === 'list') return (token.items as Tokens.ListItem[]).map(tokenText).join('\n\n')
  if (token.type === 'table') return [token.header, ...token.rows].map((row: Tokens.TableCell[]) => row.map(cell => cell.tokens.map(tokenText).join('')).join('\t')).join('\n\n')
  if (token.type === 'code' || token.type === 'codespan') return token.text
  if ('tokens' in token && token.tokens) return token.tokens.map(tokenText).join(token.type === 'blockquote' || token.type === 'list_item' ? '\n\n' : '')
  return 'text' in token && typeof token.text === 'string' ? decodeEntities(token.text) : ''
}

export function noteText(markdown: string): string {
  return parser.lexer(markdown).map(tokenText).join('\n\n').replace(/[\u200b\ufeff]/g, '').trim()
}

/** Chinese/CJK characters count individually; other letters/numbers form words. */
export function textStatistics(text: string) {
  const normalized = text.replace(/\r\n?/g, '\n').replace(/[\u200b\ufeff]/g, '')
  const cjkCount = (normalized.match(cjk) ?? []).length
  const otherWords = (normalized.replace(cjk, ' ').match(word) ?? []).length
  let characters = 0, charactersWithSpaces = 0
  const segments = graphemes ? graphemes.segment(normalized) : Array.from(normalized, segment => ({ segment }))
  for (const { segment } of segments) {
    if (!/^\s+$/u.test(segment)) characters++
    if (!/[\r\n]/.test(segment)) charactersWithSpaces++
  }
  return {
    words: cjkCount + otherWords,
    characters,
    charactersWithSpaces,
    paragraphs: normalized.split(/\n\s*\n/).filter(paragraph => paragraph.trim()).length,
    readingMinutes: characters ? Math.max(1, Math.ceil(cjkCount / 300 + otherWords / 200)) : 0,
  }
}

/** Preserve block boundaries while reusing the annotation index's exclusions. */
export function noteSelectionText(root: HTMLElement, range: Range): string {
  const selected = markdownSelection(root, range)
  if (!selected) return ''
  const parts: string[] = []
  let previous: Element | null | undefined
  for (const entry of markdownTextIndex(root).nodes) {
    if (entry.end <= selected.start || entry.start >= selected.end) continue
    const block = entry.node.parentElement?.closest(blockSelector)
    if (parts.length && block !== previous) parts.push('\n\n')
    parts.push(entry.node.data.slice(Math.max(0, selected.start - entry.start), Math.min(entry.node.length, selected.end - entry.start)))
    previous = block
  }
  return parts.join('')
}
