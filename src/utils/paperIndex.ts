import type { PaperIndexEntry } from '../types'

function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true
  if (!a || !b || typeof a !== 'object' || typeof b !== 'object') return false
  if (Array.isArray(a) || Array.isArray(b)) {
    return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((value, i) => sameValue(value, b[i]))
  }
  const left = a as Record<string, unknown>, right = b as Record<string, unknown>
  const keys = Object.keys(left)
  return keys.length === Object.keys(right).length && keys.every(key => Object.prototype.hasOwnProperty.call(right, key) && sameValue(left[key], right[key]))
}

/** Keep virtual-list input stable when the visible item identities/order did not change. */
export function reuseItemArray<T>(previous: T[] | undefined, next: T[]): T[] {
  return previous?.length === next.length && next.every((item, i) => item === previous[i]) ? previous : next
}

/** Apply real disk changes to reactive rows without replacing every row on each poll.
 * Compare complete values rather than mtime alone: extraction/status and local edits
 * may change independently of meta.json. A different slug/UUID is a different entry.
 */
export function reconcilePaperIndex(previous: PaperIndexEntry[], incoming: PaperIndexEntry[]): PaperIndexEntry[] {
  const bySlug = new Map(previous.map(entry => [entry.slug, entry]))
  const next = incoming.map(entry => {
    const current = bySlug.get(entry.slug)
    if (!current || current.id !== entry.id) return entry
    const target = current as unknown as Record<string, unknown>, source = entry as unknown as Record<string, unknown>
    for (const key of Object.keys(target)) if (!Object.prototype.hasOwnProperty.call(source, key)) delete target[key]
    for (const key of Object.keys(source)) if (!sameValue(target[key], source[key])) target[key] = source[key]
    return current
  })
  return reuseItemArray(previous, next)
}
