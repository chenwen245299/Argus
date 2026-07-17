import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { JournalRank } from '../types'

/** Normalize a venue into a cache key. Must match the Rust `normalize_venue_key`:
 *  drop 4-digit year tokens so different years of a venue share one entry. */
export function normVenue(v: string): string {
  const isYear = (tok: string) => {
    const digits = tok.replace(/\D/g, '')
    return digits.length === 4 && (digits.startsWith('19') || digits.startsWith('20'))
  }
  return v.trim().split(/\s+/).filter(t => t && !isYear(t)).join(' ').toLowerCase()
}

// Library-wide journal-rank cache, keyed by normalized venue. Once a venue is
// looked up ("ICLR 2026 → CCF A"), every paper with that venue reuses it — no
// repeat API calls. Persisted on the backend in `.argus/venue_ranks.json`.
export const useRanksStore = defineStore('ranks', () => {
  const venueRanks = ref<Record<string, JournalRank>>({})
  const loaded = ref(false)
  const inflight = new Set<string>()

  async function load() {
    try {
      venueRanks.value = await invoke<Record<string, JournalRank>>('get_venue_ranks')
    } catch (e) {
      console.error('Failed to load venue ranks:', e)
    } finally {
      loaded.value = true
    }
  }

  function get(venue?: string | null): JournalRank | undefined {
    const v = venue?.trim()
    if (!v) return undefined
    return venueRanks.value[normVenue(v)]
  }

  /** Whether we already have a (positive or negative) cached result for a venue. */
  function has(venue?: string | null): boolean {
    const v = venue?.trim()
    if (!v) return false
    return normVenue(v) in venueRanks.value
  }

  /** Fetch a venue's rank (reusing the cache unless `force`). Throws on error. */
  async function fetchRank(venue: string, force = false): Promise<JournalRank> {
    const v = venue.trim()
    const key = normVenue(v)
    if (!force && key in venueRanks.value) return venueRanks.value[key]
    if (inflight.has(key)) {
      return venueRanks.value[key] ?? { official: {}, custom: [], venue: v, fetched_at: '' }
    }
    inflight.add(key)
    try {
      const rank = await invoke<JournalRank>('fetch_journal_rank', { venue: v, force })
      venueRanks.value = { ...venueRanks.value, [key]: rank }
      return rank
    } finally {
      inflight.delete(key)
    }
  }

  return { venueRanks, loaded, load, get, has, fetchRank }
})
