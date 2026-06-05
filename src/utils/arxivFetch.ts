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

const PAGE_SIZE = 500
const TIMEOUT_MS = 120_000

async function fetchPage(url: string, cat: string): Promise<ArxivPaper[]> {
  for (let attempt = 1; attempt <= 3; attempt++) {
    let res: Response
    try {
      res = await Promise.race([
        fetch(url, {
          method: 'GET',
          headers: { 'User-Agent': 'Argus/0.1 (mailto:chengwen@comp.nus.edu.sg)' },
        }),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new Error(`arXiv request timed out after ${TIMEOUT_MS / 1000}s`)), TIMEOUT_MS)
        ),
      ])
    } catch (e) {
      if (attempt >= 3) throw e
      await new Promise(r => setTimeout(r, 5000 * attempt))
      continue
    }

    if (res.status === 429 || res.status === 503) {
      if (attempt >= 3) throw new Error(`arXiv returned ${res.status} for ${cat} after 3 attempts`)
      const wait = parseInt(res.headers.get('retry-after') ?? '') || 30 * attempt
      await new Promise(r => setTimeout(r, wait * 1000))
      continue
    }
    if (!res.ok) throw new Error(`arXiv API returned ${res.status} for ${cat}`)
    return parseAtomXml(await res.text(), new Date().toISOString())
  }
  return []
}

async function fetchCategory(cat: string, dateFrom: string, dateTo: string): Promise<ArxivPaper[]> {
  const from = dateFrom.replace(/-/g, '') + '0000'
  const to = dateTo.replace(/-/g, '') + '2359'
  const base = `https://export.arxiv.org/api/query?search_query=(cat:${cat})+AND+submittedDate:[${from}+TO+${to}]&sortBy=submittedDate&sortOrder=descending`

  const all: ArxivPaper[] = []
  let start = 0

  while (true) {
    const url = `${base}&start=${start}&max_results=${PAGE_SIZE}`
    const page = await fetchPage(url, cat)
    all.push(...page)
    if (page.length < PAGE_SIZE) break  // no more results
    start += PAGE_SIZE
    await new Promise(r => setTimeout(r, 3000))  // polite delay between pages
  }

  return all
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
    const papers = await fetchCategory(cats[i], dateFrom, dateTo)
    for (const p of papers) {
      if (!seen.has(p.arxiv_id)) { seen.add(p.arxiv_id); all.push(p) }
    }
    if (i + 1 < cats.length) await new Promise(r => setTimeout(r, 3000))
  }

  await emit('arxiv-fetch', { status: 'done', done: all.length, total: all.length })
  return all
}
