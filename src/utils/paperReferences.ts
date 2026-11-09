import { invoke } from '@tauri-apps/api/core'
import { emitTo } from '@tauri-apps/api/event'

export interface ReferencePaper {
  id: string; slug: string; title: string; citeKey: string | null
  authors: string[]; year: number | null; fileType: string | null
}
export const PAPER_REFERENCE_PREFIX = '#argus-paper='

export function referenceId(href: string): string | null {
  if (!href.startsWith(PAPER_REFERENCE_PREFIX)) return null
  try {
    const id = decodeURIComponent(href.slice(PAPER_REFERENCE_PREFIX.length))
    return id && id.length <= 512 && !/[\s\u0000-\u001f]/u.test(id) ? id : null
  } catch { return null }
}
export function referenceMarkdown(paper: ReferencePaper): string {
  const label = paper.title
  return `[${label.replace(/[\\\[\]`<>]/g, '\\$&').replace(/[\r\n]+/g, ' ')}](${PAPER_REFERENCE_PREFIX}${encodeURIComponent(paper.id)})`
}
export function matchingReferences(papers: ReferencePaper[], query: string): ReferencePaper[] {
  const q = query.trim().toLocaleLowerCase()
  const terms = q.split(/\s+/).filter(Boolean)
  const score = (paper: ReferencePaper) => {
    const key = paper.citeKey?.toLocaleLowerCase() ?? ''
    if (!q) return 0
    if (key && key === q) return 0
    if (key && key.startsWith(q)) return 1
    if (key && key.includes(q)) return 2
    const title = paper.title.toLocaleLowerCase()
    if (terms.every(term => title.includes(term))) return 3
    return 99
  }
  return papers.map(paper => ({ paper, rank: score(paper) })).filter(item => item.rank < 99)
    .sort((a, b) => a.rank - b.rank || a.paper.title.localeCompare(b.paper.title))
    .map(item => item.paper)
}

/** Plain-text note fields keep their formatting; render only our citation links. */
export function renderReferenceText(text: string): string {
  installPaperReferenceNavigation()
  const escape = (value: string) => value.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]!)
  const pattern = /\[((?:\\.|[^\]\\])*)\]\((#argus-paper=[^)\s]+)\)/g
  let html = '', end = 0
  for (const match of text.matchAll(pattern)) {
    if (!referenceId(match[2])) continue
    html += escape(text.slice(end, match.index))
      + `<a class="note-paper-reference" href="${escape(match[2])}">${escape(match[1].replace(/\\([\\\[\]`<>])/g, '$1'))}</a>`
    end = match.index! + match[0].length
  }
  return html + escape(text.slice(end))
}

let installed = false
/** Fragment links remain portable Markdown and need no custom WebView protocol. */
export function installPaperReferenceNavigation() {
  if (installed || typeof document === 'undefined') return
  installed = true
  document.addEventListener('click', event => {
    const target = event.target instanceof Element ? event.target : null
    const anchor = target?.closest('a,[data-type="a"]')
    if (!anchor) return
    const href = anchor.getAttribute('href') ?? anchor.querySelector('.vditor-ir__marker--link')?.textContent ?? ''
    const id = referenceId(href.trim())
    if (!id) return
    event.preventDefault(); event.stopImmediatePropagation()
    void (async () => {
      const root = await invoke<string | null>('get_current_library')
      if (!root) throw new Error('No library is open')
      const paper = await invoke<ReferencePaper>('resolve_note_reference', { paperId: id, expectedRoot: root })
      if (await invoke<string | null>('get_current_library') !== root) throw new Error('Library changed')
      await emitTo('main', 'argus-open-paper', { slug: paper.slug, title: paper.title, root })
      await invoke('focus_main_window')
    })().catch(error => {
      window.dispatchEvent(new CustomEvent('argus-note-reference-error', { detail: String(error) }))
    })
  }, true)
}
