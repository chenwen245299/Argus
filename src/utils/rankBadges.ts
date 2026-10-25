import type { JournalRank } from '../types'

// The rank categories we surface as badges, in display order: 影响因子、CCF、
// SCI 分区、中科院分区、EI. A value can come from an official easyScholar dataset
// OR from a user's custom dataset (customRank) — e.g. CCF is often configured as
// a custom dataset, so a conference like NeurIPS returns "CCF A" only there.
export interface RankCat {
  label: string
  color: string
  officialKeys: string[]   // easyScholar official field codes
  customAliases: string[]  // lowercased customRank abbName matches
  labelOnly?: boolean
}

export const RANK_CATS: RankCat[] = [
  { label: 'IF',    color: '#16a34a', officialKeys: ['sciif'],            customAliases: ['if', '影响因子'] },
  { label: 'CCF',   color: '#e11d48', officialKeys: ['ccf'],              customAliases: ['ccf'] },
  { label: 'SCI',   color: '#7c3aed', officialKeys: ['sci'],              customAliases: ['sci', 'sci分区'] },
  { label: '中科院', color: '#ea580c', officialKeys: ['sciUp', 'sciBase'], customAliases: ['中科院', '中科院分区', '中科院升级版', '中科院升级版分区'] },
  { label: 'EI',    color: '#0d9488', officialKeys: ['eii'],              customAliases: ['ei', 'ei检索'], labelOnly: true },
]

/** Submission/preprint status is not acceptance. Keep in sync with the Rust
 *  publication::is_unpublished_venue; check before using any cached rank. */
export function isUnpublishedVenue(venue?: string | null): boolean {
  return !!venue && /\b(?:under[\s-]+review|in[\s-]+review|awaiting[\s-]+decision|pending[\s-]+review|submission|submitted|withdrawn|rejected|desk[\s-]+reject(?:ed)?|not[\s-]+accepted|acceptance[\s-]+unconfirmed|pre[\s-]?print|arxiv|biorxiv|medrxiv)\b|在审|审稿中|待审|投稿中|已投稿|未录用|拒稿|撤稿|预印本/i.test(venue.replace(/_/g, ' '))
}

export interface DisplayBadge { text: string; color: string }

export function badgesFromRank(rank: JournalRank | null | undefined): DisplayBadge[] {
  if (!rank || isUnpublishedVenue(rank.venue)) return []
  const official = rank.official ?? {}
  const custom = rank.custom ?? []
  const out: DisplayBadge[] = []
  for (const cat of RANK_CATS) {
    // Prefer an official dataset value, else a matching custom dataset.
    let value = ''
    for (const k of cat.officialKeys) {
      const v = String(official[k] ?? '').trim()
      if (v) { value = v; break }
    }
    if (!value) {
      const c = custom.find(c => cat.customAliases.includes((c.name ?? '').trim().toLowerCase()))
      if (c) value = String(c.rank ?? '').trim()
    }
    if (!value) continue
    const text = cat.labelOnly || value === cat.label ? cat.label : `${cat.label} ${value}`
    out.push({ text, color: cat.color })
  }
  return out
}
