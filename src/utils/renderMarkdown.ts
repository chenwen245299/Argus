import { marked, type Tokens } from 'marked'
import hljs from 'highlight.js/lib/common'
import DOMPurify from 'dompurify'
import katex from 'katex'

// common build covers: js/ts/python/java/go/rust/cpp/c/csharp/bash/shell/json/xml/css/sql/yaml/etc.
// Register a few convenient aliases not in the common map.
hljs.registerAliases(['js'], { languageName: 'javascript' })
hljs.registerAliases(['ts'], { languageName: 'typescript' })
hljs.registerAliases(['py'], { languageName: 'python' })
hljs.registerAliases(['rb'], { languageName: 'ruby' })
hljs.registerAliases(['sh'], { languageName: 'bash' })
hljs.registerAliases(['md'], { languageName: 'markdown' })
hljs.registerAliases(['yml'], { languageName: 'yaml' })
hljs.registerAliases(['cs'], { languageName: 'csharp' })
hljs.registerAliases(['c++'], { languageName: 'cpp' })
hljs.registerAliases(['html'], { languageName: 'xml' })

const COPY_ICON = `<svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>`
let handlersInstalled = false

function installMarkdownHandlers() {
  if (handlersInstalled || typeof document === 'undefined') return
  handlersInstalled = true

  document.addEventListener('click', (event) => {
    const target = event.target as HTMLElement | null
    const button = target?.closest<HTMLButtonElement>('[data-md-action]')
    if (!button) return

    const action = button.dataset.mdAction
    if (action === 'copy-code') {
      const code = button.closest('.md-code-block')?.querySelector('code')
      const text = code?.textContent ?? ''
      if (!text) return
      navigator.clipboard?.writeText(text).catch(() => {})
      button.textContent = '已复制'
      window.setTimeout(() => { button.innerHTML = COPY_ICON }, 2000)
      return
    }

    if (action === 'toggle-svg-source') {
      const pre = button.closest('.svg-code-block')?.querySelector<HTMLElement>('.svg-source-pre')
      if (!pre) return
      const nextVisible = pre.style.display === 'none'
      pre.style.display = nextVisible ? 'block' : 'none'
      button.textContent = nextVisible ? '隐藏代码' : '查看代码'
    }
  })
}

function escapeHtmlAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

marked.use({
  breaks: true,
  renderer: {
    code(token: Tokens.Code): string {
      const { text, lang } = token
      if (lang === 'svg') {
        const clean = DOMPurify.sanitize(text, { USE_PROFILES: { svg: true, svgFilters: true } })
        const highlighted = hljs.highlight(text, { language: 'xml' }).value
        return `<div class="md-code-block svg-code-block"><div class="md-code-header"><span class="md-code-lang">svg</span><button class="svg-toggle-btn" data-md-action="toggle-svg-source" type="button">查看代码</button><button class="md-copy-btn" data-action="copy-svg-image" title="复制图片" type="button">${COPY_ICON}</button></div><div class="svg-preview-area">${clean}</div><pre class="svg-source-pre" style="display:none"><code class="hljs xml">${highlighted}</code></pre></div>`
      }
      const validLang = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
      const highlighted = hljs.highlight(text, { language: validLang }).value
      return `<div class="md-code-block"><div class="md-code-header"><span class="md-code-lang">${escapeHtml(validLang)}</span><button class="md-copy-btn" data-md-action="copy-code" type="button">${COPY_ICON}</button></div><pre><code class="hljs ${escapeHtmlAttr(validLang)}">${highlighted}</code></pre></div>`
    },
    codespan(token: Tokens.Codespan): string {
      return `<code class="md-inline-code">${escapeHtml(token.text)}</code>`
    },
    image(token: Tokens.Image): string {
      const safeHref = escapeHtmlAttr(token.href)
      const safeAlt = escapeHtmlAttr(token.text)
      return `<img src="${safeHref}" alt="${safeAlt}" class="md-img" style="max-width:100%;height:auto;border-radius:8px;display:block;margin:6px 0" />`
    },
    link(token: Tokens.Link): string {
      const safeHref = escapeHtmlAttr(token.href)
      const safeTitle = token.title ? ` title="${escapeHtmlAttr(token.title)}"` : ''
      return `<a href="${safeHref}"${safeTitle} class="md-link" target="_blank" rel="noopener">${token.text}</a>`
    },
  },
})

export function renderMarkdown(content: string): string {
  try {
    installMarkdownHandlers()
    const codeBlocks: string[] = []
    let md = content.replace(/```[\s\S]*?```/g, (m) => {
      codeBlocks.push(m)
      return `<!--CODE_BLOCK_${codeBlocks.length - 1}-->`
    })

    const inlineCodes: string[] = []
    md = md.replace(/`[^`]+`/g, (m) => {
      inlineCodes.push(m)
      return `<!--INLINE_CODE_${inlineCodes.length - 1}-->`
    })

    const displayMaths: string[] = []
    md = md.replace(/\\\[([\s\S]+?)\\\]/g, (_, tex) => {
      displayMaths.push(tex.trim())
      return `<!--DISPLAY_MATH_${displayMaths.length - 1}-->`
    })
    md = md.replace(/\$\$([\s\S]+?)\$\$/g, (_, tex) => {
      displayMaths.push(tex.trim())
      return `<!--DISPLAY_MATH_${displayMaths.length - 1}-->`
    })

    const inlineMaths: string[] = []
    md = md.replace(/\\\(([\s\S]+?)\\\)/g, (_, tex) => {
      inlineMaths.push(tex.trim())
      return `<!--INLINE_MATH_${inlineMaths.length - 1}-->`
    })
    let mi = 0
    while (mi < md.length) {
      const start = md.indexOf('$', mi)
      if (start === -1) break
      if (md[start + 1] === '$') { mi = start + 2; continue }
      const end = md.indexOf('$', start + 1)
      if (end === -1) break
      if (md[end + 1] === '$') { mi = end + 2; continue }
      const tex = md.slice(start + 1, end).trim()
      inlineMaths.push(tex)
      const ph = `<!--INLINE_MATH_${inlineMaths.length - 1}-->`
      md = md.slice(0, start) + ph + md.slice(end + 1)
      mi = start + ph.length
    }

    let html = marked.parse(md) as string
    if (typeof html !== 'string') return content

    html = html.replace(/<!--DISPLAY_MATH_(\d+)-->/g, (_, i) => {
      try {
        return katex.renderToString(displayMaths[+i], { displayMode: true, throwOnError: false, strict: false })
      } catch {
        return escapeHtml(`$$${displayMaths[+i]}$$`)
      }
    })

    html = html.replace(/<!--INLINE_MATH_(\d+)-->/g, (_, i) => {
      try {
        return katex.renderToString(inlineMaths[+i], { displayMode: false, throwOnError: false, strict: false })
      } catch {
        return escapeHtml(`$${inlineMaths[+i]}$`)
      }
    })

    html = html.replace(/<!--CODE_BLOCK_(\d+)-->/g, (_, i) => {
      return marked.parse(codeBlocks[+i]) as string
    })

    html = html.replace(/<!--INLINE_CODE_(\d+)-->/g, (_, i) => {
      const raw = inlineCodes[+i].slice(1, -1)
      return `<code class="md-inline-code">${escapeHtml(raw)}</code>`
    })

    return DOMPurify.sanitize(html, {
      ADD_ATTR: ['data-action', 'data-md-action', 'target', 'rel', 'title', 'type'],
      ADD_TAGS: ['svg', 'path', 'circle', 'rect', 'line', 'polyline', 'polygon', 'g', 'defs', 'use', 'symbol'],
      FORCE_BODY: false,
    })
  } catch {
    return content
  }
}

export type Seg = { type: 'md'; html: string } | { type: 'mermaid'; src: string }

export function getSegments(content: string, streaming = false): Seg[] {
  if (!content) return []
  if (streaming) return [{ type: 'md', html: renderMarkdown(content) }]
  const segs: Seg[] = []
  const re = /```mermaid[^\n]*\n([\s\S]*?)```/g
  let last = 0
  for (const m of content.matchAll(re)) {
    if (m.index! > last) segs.push({ type: 'md', html: renderMarkdown(content.slice(last, m.index!)) })
    segs.push({ type: 'mermaid', src: m[1].trim() })
    last = m.index! + m[0].length
  }
  if (last < content.length) segs.push({ type: 'md', html: renderMarkdown(content.slice(last)) })
  return segs.length ? segs : [{ type: 'md', html: renderMarkdown(content) }]
}
