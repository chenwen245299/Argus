import { fetch } from '@tauri-apps/plugin-http'
import { emit } from '@tauri-apps/api/event'
import type { ArxivConfig, ArxivPaper } from '../types'

function parseAtomXml(xml: string, fetchedAt: string): ArxivPaper[] {
  const parser = new DOMParser()
  const doc = parser.parseFromString(xml, 'application/xml')
  const entries = Array.from(doc.querySelectorAll('entry'))

  return entries.flatMap(entry => {
    const idText = entry.querySelector('id')?.textContent ?? ''
    const arxivId = idText.split('/abs/')[1]?.replace(/v\d+$/, '') ?? ''
    if (!arxivId) return []

    const title = (entry.querySelector('title')?.textContent ?? '').trim().replace(/\s+/g, ' ')
    const summary = (entry.querySelector('summary')?.textContent ?? '').trim()
    const published = entry.querySelector('published')?.textContent ?? ''
    const updated = entry.querySelector('updated')?.textContent ?? ''
    const authors = Array.from(entry.querySelectorAll('author name'))
      .map(n => n.textContent?.trim() ?? '').filter(Boolean)
    const categories = [...new Set(
      Array.from(entry.querySelectorAll('category'))
        .map(c => c.getAttribute('term') ?? '').filter(Boolean)
    )]

    const links = Array.from(entry.querySelectorAll('link'))
    const pdfLink = links.find(l => l.getAttribute('title') === 'pdf' || (l.getAttribute('href') ?? '').includes('/pdf/'))
    const absLink = links.find(l => (l.getAttribute('type') ?? '').includes('text/html') || (l.getAttribute('href') ?? '').includes('/abs/'))
    const pdf_url = pdfLink?.getAttribute('href') ?? `https://arxiv.org/pdf/${arxivId}`
    const abs_url = absLink?.getAttribute('href') ?? `https://arxiv.org/abs/${arxivId}`

    return [{
      arxiv_id: arxivId, title, authors, summary, categories,
      published, updated, pdf_url, abs_url,
      relevance_score: null, relevance_reason: null,
      key_contributions: [], analysis_summary: null, matched_topics: [],
      analysis_status: 'pending', in_library: false, fetched_at: fetchedAt,
      read: false, rating: 0,
    } satisfies ArxivPaper]
  })
}

async function fetchCategory(cat: string, dateFrom: string, dateTo: string, maxResults: number): Promise<ArxivPaper[]> {
  const from = dateFrom.replace(/-/g, '') + '0000'
  const to = dateTo.replace(/-/g, '') + '2359'
  const url = `https://export.arxiv.org/api/query?search_query=(cat:${cat})AND+submittedDate:[${from}+TO+${to}]&start=0&max_results=${maxResults}&sortBy=submittedDate&sortOrder=descending`

  for (let attempt = 1; attempt <= 3; attempt++) {
    const res = await fetch(url, {
      method: 'GET',
      headers: { 'User-Agent': 'Argus/0.1 (local research tool)' },
    })
    if (res.status === 429) {
      if (attempt >= 3) throw new Error(`arXiv rate-limited on category ${cat}`)
      const wait = parseInt(res.headers.get('retry-after') ?? '') || 60 * attempt
      await new Promise(r => setTimeout(r, wait * 1000))
      continue
    }
    if (!res.ok) throw new Error(`arXiv API returned ${res.status} for ${cat}`)
    const xml = await res.text()
    return parseAtomXml(xml, new Date().toISOString())
  }
  return []
}

export async function fetchArxivCategories(
  config: ArxivConfig,
  dateFrom: string,
  dateTo: string,
): Promise<ArxivPaper[]> {
  const cats = config.categories.length > 0 ? config.categories : ['cs.CL']
  const seen = new Set<string>()
  const all: ArxivPaper[] = []

  for (let i = 0; i < cats.length; i++) {
    await emit('arxiv-fetch', {
      status: 'fetching', done: i, total: cats.length,
      message: `正在抓取 ${cats[i]} (${i + 1}/${cats.length})`,
    })
    const papers = await fetchCategory(cats[i], dateFrom, dateTo, config.max_fetch)
    for (const p of papers) {
      if (!seen.has(p.arxiv_id)) { seen.add(p.arxiv_id); all.push(p) }
    }
    if (i + 1 < cats.length) await new Promise(r => setTimeout(r, 3000))
  }

  await emit('arxiv-fetch', { status: 'done', done: all.length, total: all.length })
  return all
}
