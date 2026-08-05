// Copying a selection out of rendered markdown should yield markdown source.
//
// The motivating case is math: KaTeX's visual layer is a pile of positioned
// spans, so a plain browser copy of $x_i$ hands back "xi" — glyphs in DOM order,
// not something you can paste into a note or a prompt. The TeX source lives in
// the MathML <annotation>, so we intercept `copy`, walk the selected fragment and
// serialize it back to markdown (formulas, emphasis, links, lists, code fences).
//
// Only text/plain is written: the point is to get source, so rich-text targets
// should fall back to the markdown too rather than paste rendered HTML.

// Surfaces rendered by renderMarkdown()/getSegments(). Keep in sync when a new
// rendered-markdown container appears.
const MD_CONTAINERS = '.markdown-body, .hl-note-text, .hl-note, .notes-preview'

const TEX_ANNOTATION = 'annotation[encoding="application/x-tex"]'

type Ctx = {
  /** Nesting of ul/ol, innermost last. */
  lists: Array<{ ordered: boolean; index: number }>
  /** Inside <pre>: preserve whitespace verbatim. */
  pre: boolean
}

function texOf(el: Element): string | null {
  const tex = el.querySelector(TEX_ANNOTATION)?.textContent
  return tex ? tex.trim() : null
}

/** Visible glyphs only — used when a partial selection clipped the annotation. */
function katexFallbackText(el: Element): string {
  const html = el.querySelector('.katex-html')
  return (html?.textContent ?? el.textContent ?? '').replace(/\s+/g, ' ').trim()
}

function codeBlockOf(el: Element): string {
  const lang = el.querySelector('.md-code-lang')?.textContent?.trim() ?? ''
  // The SVG variant keeps its source in a separate, normally hidden <pre>.
  const code = el.querySelector('.svg-source-pre code') ?? el.querySelector('pre code')
  const body = (code?.textContent ?? '').replace(/\n+$/, '')
  return `\n\n\`\`\`${lang}\n${body}\n\`\`\`\n\n`
}

function tableOf(el: Element): string {
  const rows = [...el.querySelectorAll('tr')]
  if (!rows.length) return ''
  // Cells go through the walker too — textContent would flatten a formula into
  // its MathML-plus-glyphs soup instead of emitting $…$.
  const cellsOf = (tr: Element) =>
    [...tr.children].map(td =>
      childrenOf(td, { lists: [], pre: false }).replace(/\s+/g, ' ').trim().replace(/\|/g, '\\|'))
  const head = cellsOf(rows[0])
  const lines = [`| ${head.join(' | ')} |`, `| ${head.map(() => '---').join(' | ')} |`]
  for (const tr of rows.slice(1)) lines.push(`| ${cellsOf(tr).join(' | ')} |`)
  return `\n\n${lines.join('\n')}\n\n`
}

function childrenOf(node: Node, ctx: Ctx): string {
  let out = ''
  for (const child of node.childNodes) out += serialize(child, ctx)
  return out
}

function serialize(node: Node, ctx: Ctx): string {
  if (node.nodeType === Node.TEXT_NODE) {
    const text = node.nodeValue ?? ''
    // Outside <pre>, HTML collapses whitespace — mirror that so marked's source
    // indentation doesn't leak into the copied text.
    return ctx.pre ? text : text.replace(/\s+/g, ' ')
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return ''

  const el = node as Element
  const tag = el.tagName.toLowerCase()

  // ── Math ──
  if (el.classList.contains('katex-display')) {
    const inner = el.querySelector('.katex')
    const tex = inner ? texOf(inner) : texOf(el)
    return tex ? `\n\n$$${tex}$$\n\n` : `\n\n${katexFallbackText(el)}\n\n`
  }
  if (el.classList.contains('katex')) {
    const tex = texOf(el)
    return tex ? `$${tex}$` : katexFallbackText(el)
  }
  // Hidden MathML twin; the .katex branch above already emitted the source.
  if (el.classList.contains('katex-mathml')) return ''

  // ── Code ──
  if (el.classList.contains('md-code-block')) return codeBlockOf(el)
  if (el.classList.contains('md-code-header')) return ''
  if (tag === 'pre') {
    const body = (el.textContent ?? '').replace(/\n+$/, '')
    return `\n\n\`\`\`\n${body}\n\`\`\`\n\n`
  }
  if (tag === 'code') {
    if (ctx.pre) return childrenOf(el, ctx)
    const text = (el.textContent ?? '').replace(/\s+/g, ' ')
    // A backtick in the content needs a longer fence to stay valid.
    const fence = '`'.repeat(Math.max(...[...text.matchAll(/`+/g)].map(m => m[0].length), 0) + 1)
    return `${fence}${text}${fence}`
  }

  // ── Inline ──
  if (tag === 'br') return '\n'
  if (tag === 'strong' || tag === 'b') return `**${childrenOf(el, ctx)}**`
  if (tag === 'em' || tag === 'i') return `*${childrenOf(el, ctx)}*`
  if (tag === 'del' || tag === 's') return `~~${childrenOf(el, ctx)}~~`
  if (tag === 'a') {
    const href = el.getAttribute('href') ?? ''
    const text = childrenOf(el, ctx)
    return href ? `[${text}](${href})` : text
  }
  if (tag === 'img') {
    const src = el.getAttribute('src') ?? ''
    return `![${el.getAttribute('alt') ?? ''}](${src})`
  }

  // ── Blocks ──
  if (tag === 'hr') return '\n\n---\n\n'
  if (/^h[1-6]$/.test(tag)) return `\n\n${'#'.repeat(+tag[1])} ${childrenOf(el, ctx).trim()}\n\n`
  if (tag === 'p') return `\n\n${childrenOf(el, ctx).trim()}\n\n`
  if (tag === 'blockquote') {
    const body = childrenOf(el, ctx).trim()
    return `\n\n${body.split('\n').map(l => (l ? `> ${l}` : '>')).join('\n')}\n\n`
  }
  if (tag === 'table') return tableOf(el)
  if (tag === 'ul' || tag === 'ol') {
    ctx.lists.push({ ordered: tag === 'ol', index: Number(el.getAttribute('start') ?? 1) })
    const body = childrenOf(el, ctx)
    ctx.lists.pop()
    // Nested lists attach to their parent item; only the outermost breaks blocks.
    return ctx.lists.length ? body : `\n\n${body.trim()}\n\n`
  }
  if (tag === 'li') {
    const list = ctx.lists[ctx.lists.length - 1]
    const depth = Math.max(0, ctx.lists.length - 1)
    const indent = '  '.repeat(depth)
    const marker = list?.ordered ? `${list.index++}.` : '-'
    const body = childrenOf(el, ctx).trim()
    // Continuation lines line up under the marker so nesting survives a re-parse.
    const pad = ' '.repeat(marker.length + 1)
    const text = body.split('\n').map((l, i) => (i === 0 || !l ? l : `${indent}${pad}${l}`)).join('\n')
    return `\n${indent}${marker} ${text}`
  }

  return childrenOf(el, ctx)
}

export function fragmentToMarkdown(fragment: DocumentFragment | Element): string {
  const ctx: Ctx = { lists: [], pre: false }
  // Text nodes are already whitespace-collapsed as they are emitted; do NOT add a
  // global space squeeze here — it would eat code-fence and nested-list indents.
  return childrenOf(fragment, ctx)
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

/** True when the whole selection sits inside one rendered-markdown container. */
function selectionContainer(sel: Selection): Element | null {
  if (sel.rangeCount !== 1 || sel.isCollapsed) return null
  const range = sel.getRangeAt(0)
  const node = range.commonAncestorContainer
  const el = node.nodeType === Node.ELEMENT_NODE ? (node as Element) : node.parentElement
  return el?.closest(MD_CONTAINERS) ?? null
}

export function installMarkdownCopy() {
  if (typeof document === 'undefined') return
  document.addEventListener('copy', (event: ClipboardEvent) => {
    const sel = window.getSelection()
    if (!sel || !selectionContainer(sel)) return

    let md: string
    try {
      md = fragmentToMarkdown(sel.getRangeAt(0).cloneContents())
    } catch {
      return   // fall through to the browser's own copy
    }
    if (!md) return

    event.clipboardData?.setData('text/plain', md)
    event.preventDefault()
  })
}
