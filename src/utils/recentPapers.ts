import { ref } from 'vue'
import type { PaperIndexEntry } from '../types'

const RECENT_PAPERS_KEY = 'argus:recent-paper-slugs'
const MAX_RECENT_PAPERS = 500

// Reactive signal that bumps every time a paper access is recorded. Views that
// derive from the recent list (e.g. the "Recently Read" nav) read this to stay
// reactive, since the underlying data lives in localStorage.
export const recentPapersVersion = ref(0)

export function recentPaperSlugs(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_PAPERS_KEY)
    const parsed = raw ? JSON.parse(raw) : []
    return Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === 'string') : []
  } catch {
    return []
  }
}

export function recordPaperAccess(slug: string | null | undefined) {
  if (!slug) return
  try {
    const next = [slug, ...recentPaperSlugs().filter(s => s !== slug)].slice(0, MAX_RECENT_PAPERS)
    localStorage.setItem(RECENT_PAPERS_KEY, JSON.stringify(next))
  } catch {}
  recentPapersVersion.value++
}

export function sortPapersByRecentAccess<T extends Pick<PaperIndexEntry, 'slug'>>(papers: T[]): T[] {
  const rank = new Map(recentPaperSlugs().map((slug, idx) => [slug, idx]))
  return papers
    .map((paper, idx) => ({ paper, idx, rank: rank.get(paper.slug) ?? Number.POSITIVE_INFINITY }))
    .sort((a, b) => (a.rank - b.rank) || (a.idx - b.idx))
    .map(item => item.paper)
}

// Keep only papers that have actually been opened, ordered most-recent-first.
export function filterRecentlyRead<T extends Pick<PaperIndexEntry, 'slug'>>(papers: T[]): T[] {
  const rank = new Map(recentPaperSlugs().map((slug, idx) => [slug, idx]))
  return papers
    .filter(p => rank.has(p.slug))
    .sort((a, b) => (rank.get(a.slug)! - rank.get(b.slug)!))
}
