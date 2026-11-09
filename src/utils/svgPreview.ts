import DOMPurify from 'dompurify'
import { normalizeSvgDimensions } from './svgToPng'

/** Keep SVG out of the Markdown parser and the surrounding document's CSS/IDs. */
export function svgPreviewUrl(source: string): string | null {
  if (!/^\s*(?:<\?xml[^>]*>\s*)?<svg\b/i.test(source)
    || (!/<\/svg\s*>\s*$/i.test(source) && !/^\s*<svg\b(?:"[^"]*"|'[^']*'|[^'"<>])*\/\s*>\s*$/i.test(source))) return null

  // Use the SVG profile, never HTML/foreignObject. Local <use> references are
  // useful for diagrams; remote references are removed below.
  const fragment = DOMPurify.sanitize(source, {
    USE_PROFILES: { svg: true, svgFilters: true },
    ADD_TAGS: ['use'],
    RETURN_DOM_FRAGMENT: true,
  })
  const svg = fragment.querySelector('svg')
  if (!svg) return null
  for (const el of [svg, ...svg.querySelectorAll('*')]) {
    for (const attr of [...el.attributes]) {
      if ((attr.localName === 'href' && !attr.value.startsWith('#'))
        || (attr.prefix === 'xml' && attr.localName === 'base')) el.removeAttributeNode(attr)
    }
  }
  normalizeSvgDimensions(svg)
  // Image decoding keeps SVG styles and duplicate definition IDs local to this
  // image. It also prevents SVG content from accessing the app document.
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(new XMLSerializer().serializeToString(svg))}`
}

/** Extract complete, possibly nested SVGs without interpreting their whitespace. */
export function protectSvgBlocks(source: string, render: (svg: string) => string): string {
  const tags = /<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<\/?svg\b(?:"[^"]*"|'[^']*'|[^'">])*>/gi
  let depth = 0, start = 0, last = 0, result = ''
  for (const match of source.matchAll(tags)) {
    const tag = match[0]
    if (tag.startsWith('<!')) continue
    if (/^<\/svg/i.test(tag)) {
      if (!depth) continue
      depth--
    } else {
      if (!depth) start = match.index!
      if (!/\/\s*>$/.test(tag)) depth++
    }
    if (!depth) {
      const end = match.index! + tag.length
      result += source.slice(last, start) + render(source.slice(start, end))
      last = end
    }
  }
  if (depth) return result + source.slice(last, start) + render(source.slice(start))
  return result + source.slice(last)
}
