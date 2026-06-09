import { fetch } from '@tauri-apps/plugin-http'
import type { ArxivPaper } from '../types'

// bioRxiv API returns 100 results per page; cursor is the 0-based offset.
const PAGE_SIZE = 100
const TIMEOUT_MS = 60_000

interface BiorxivApiPaper {
  doi: string
  title: string
  authors: string       // semicolon-separated, e.g. "Smith J; Jones A; Lee B"
  date: string          // YYYY-MM-DD
  version: string       // "1", "2", ...
  category: string
  abstract: string
  published: string     // "-" if not published, otherwise journal DOI
  server: string        // "biorxiv"
}

// messages[0] from the API — total/count may arrive as strings or numbers
interface BiorxivMessage {
  status: string
  total: string | number
  count: string | number
  count_new_papers?: string | number
}

interface BiorxivApiResponse {
  messages: BiorxivMessage[]
  collection: BiorxivApiPaper[]
}

function toInt(v: string | number | undefined): number {
  if (v === undefined || v === null) return 0
  return typeof v === 'number' ? v : parseInt(String(v), 10) || 0
}

function mapToPaper(raw: BiorxivApiPaper, fetchedAt: string): ArxivPaper {
  const doi = raw.doi.trim()
  const version = parseInt(raw.version, 10) || 1
  const paperDate = raw.date?.trim() || fetchedAt
  return {
    arxiv_id: doi,
    title: raw.title.replace(/\s+/g, ' ').trim(),
    // Authors are semicolon-separated in the API response
    authors: raw.authors.split(/\s*;\s*/).map(a => a.trim()).filter(Boolean),
    summary: raw.abstract.trim(),
    categories: [raw.category.trim()],
    published: paperDate,
    updated: paperDate,
    pdf_url: `https://www.biorxiv.org/content/${doi}v${version}.full.pdf`,
    abs_url: `https://www.biorxiv.org/content/${doi}`,
    relevance_score: null,
    relevance_reason: null,
    key_contributions: [],
    analysis_summary: null,
    matched_topics: [],
    analysis_status: 'pending',
    in_library: false,
    // For bioRxiv, use the paper date as the inbox bucket date so a
    // multi-day backfill appears under each actual day instead of today.
    fetched_at: paperDate,
    read: false,
    rating: 0,
    source: 'biorxiv',
  }
}

async function fetchPage(
  interval: string,
  cursor: number,
): Promise<{ papers: ArxivPaper[]; total: number; pageSize: number }> {
  const url = `https://api.biorxiv.org/details/biorxiv/${interval}/${cursor}`
  const fetchedAt = new Date().toISOString()

  for (let attempt = 1; attempt <= 3; attempt++) {
    let res: Response
    try {
      res = await Promise.race([
        fetch(url, { method: 'GET', headers: { Accept: 'application/json' } }),
        new Promise<never>((_, rej) =>
          setTimeout(() => rej(new Error('bioRxiv request timed out')), TIMEOUT_MS),
        ),
      ])
    } catch (e) {
      if (attempt === 3) throw e
      await new Promise(r => setTimeout(r, attempt * 2000))
      continue
    }

    if (res.status === 429 || res.status === 503) {
      await new Promise(r => setTimeout(r, attempt * 5000))
      continue
    }
    if (!res.ok) throw new Error(`bioRxiv API returned HTTP ${res.status}`)

    const data: BiorxivApiResponse = await res.json()
    const msg = data?.messages?.[0]
    if (!msg || msg.status !== 'ok') return { papers: [], total: 0, pageSize: 0 }

    const total = toInt(msg.total)
    const collection = data.collection ?? []
    const papers = collection.map(raw => mapToPaper(raw, fetchedAt))
    return { papers, total, pageSize: collection.length }
  }
  return { papers: [], total: 0, pageSize: 0 }
}

export async function fetchBiorxivAsArxivPapers(
  dateFrom: string,
  dateTo: string,
): Promise<ArxivPaper[]> {
  const interval = `${dateFrom}/${dateTo}`
  const allPapers: ArxivPaper[] = []
  let cursor = 0
  let total = Infinity

  while (cursor < total) {
    const { papers, total: t, pageSize } = await fetchPage(interval, cursor)

    // First page returned nothing — no papers for this range
    if (cursor === 0 && t === 0) break

    total = t
    allPapers.push(...papers)
    cursor += PAGE_SIZE

    // Stop if the page was incomplete (last page)
    if (pageSize < PAGE_SIZE) break

    // Polite delay between pages
    await new Promise(r => setTimeout(r, 1000))
  }

  return allPapers
}
