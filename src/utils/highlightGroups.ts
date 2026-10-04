import type { Highlight } from '../types'
import { hasLineBreaks, highlightDisplayText } from './highlightText'

/**
 * One logical highlight, as the user made it.
 *
 * A PDF selection that crosses a page break is stored as one `Highlight` record
 * per page — each with that page's rects, the WHOLE selection text and one shared
 * `created_at` (see PdfViewer `createHighlight`). Listing the raw records shows the
 * same passage twice, and right-click → delete / colour / note only touches the
 * half that was clicked. A group stitches those records back into the single
 * highlight the user sees on screen.
 *
 * Storage is deliberately NOT changed: no new field, no migration. Older app
 * builds on a synced library keep writing these one-per-page records (and strip
 * any field they do not know), so the `(created_at, text)` identity is the only
 * marker that is always there. Groups are a derived view — never write one back,
 * and never collapse the store's own array (every raw id must stay addressable
 * for `save_highlights` / tombstones). The Rust twin is
 * `src-tauri/src/highlight_groups.rs`; keep the two rules identical.
 */
export interface HighlightGroup {
  /** Canonical member's id (the lowest page). Stable across reloads — use it as
   *  the row key, the jump target and the key for per-highlight UI state. */
  id: string
  /** All records of the selection, sorted by page, then first rect's y, then id. */
  members: Highlight[]
  ids: string[]
  /** First / last page covered. Equal for an ordinary single-page highlight. */
  page: number
  pageEnd: number
  /** The canonical member's text — never a join, every member already holds all of it.
   *  As captured, line breaks and all; show `displayText` instead. */
  text: string
  /** `text` as it reads: one paragraph, unless the user keeps the line breaks. */
  displayText: string
  /** Whether `text` has a line break at all — i.e. whether the merge toggle means anything. */
  hasLineBreaks: boolean
  /** The user chose to keep the original line breaks. From the most recently edited
   *  member, like the colour. */
  keepLineBreaks: boolean
  /** From the most recently edited member, so a group edit that only half-landed
   *  (older build, sync race) still shows what the user chose last. */
  color: string
  style: Highlight['style']
  /** Distinct non-empty notes of the members, blank-line separated. A note typed
   *  on one half is therefore never hidden by the other half being empty. */
  note: string | undefined
}

function compareMembers(a: Highlight, b: Highlight): number {
  if (a.page !== b.page) return a.page - b.page
  const ay = a.rects[0]?.y ?? 0
  const by = b.rects[0]?.y ?? 0
  if (ay !== by) return ay - by
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0
}

function mergeNotes(members: Highlight[]): string | undefined {
  const seen = new Set<string>()
  const notes: string[] = []
  for (const m of members) {
    const key = m.note?.trim()
    if (!key || seen.has(key)) continue
    seen.add(key)
    notes.push(m.note as string)
  }
  return notes.length ? notes.join('\n\n') : undefined
}

function build(members: Highlight[]): HighlightGroup {
  const sorted = members.length > 1 ? [...members].sort(compareMembers) : members
  const canonical = sorted[0]
  // Newest edit wins; on a tie the earlier member (lowest page) does.
  let newest = canonical
  for (const m of sorted) {
    if ((m.updated_at ?? m.created_at) > (newest.updated_at ?? newest.created_at)) newest = m
  }
  return {
    id: canonical.id,
    members: sorted,
    ids: sorted.map(m => m.id),
    page: canonical.page,
    pageEnd: sorted.reduce((mx, m) => Math.max(mx, m.page), canonical.page),
    text: canonical.text,
    displayText: highlightDisplayText({ ...canonical, keep_line_breaks: newest.keep_line_breaks }),
    hasLineBreaks: canonical.start_offset == null && canonical.end_offset == null && hasLineBreaks(canonical.text),
    keepLineBreaks: !!newest.keep_line_breaks,
    color: newest.color,
    style: newest.style,
    note: sorted.length > 1 ? mergeNotes(sorted) : canonical.note,
  }
}

/**
 * Collapse one-selection-per-page records into logical highlights.
 *
 * Records group when they share `created_at` AND `text` exactly and sit on at least
 * two distinct pages. Pages need not be adjacent (an unrendered middle page leaves
 * a gap). Anything else — a lone survivor whose twin was deleted, two records on
 * one page, ebook records (they carry offsets and are always one per selection) —
 * is an ordinary single-member entry. Entries keep the order of first appearance.
 */
export function groupHighlights(list: readonly Highlight[]): HighlightGroup[] {
  // Pass 1: bucket the records that may group, by (created_at, text).
  const bucketOf: (string | null)[] = []
  const buckets = new Map<string, Highlight[]>()
  for (const h of list) {
    if (h.start_offset != null || h.end_offset != null) { bucketOf.push(null); continue }
    // Exact (created_at, text) identity. JSON keeps the pair unambiguous, where a
    // joined string could collide if a field ever held the separator.
    const key = JSON.stringify([h.created_at, h.text])
    bucketOf.push(key)
    const bucket = buckets.get(key)
    if (bucket) bucket.push(h)
    else buckets.set(key, [h])
  }

  // Pass 2: walk the input in order, emitting each entry where it first appears. A
  // bucket that does not span two pages leaves its records as individual entries at
  // their own positions. (Same walk as `collapse_highlights` in Rust.)
  const emitted = new Set<string>()
  const out: HighlightGroup[] = []
  list.forEach((h, i) => {
    const key = bucketOf[i]
    const bucket = key === null ? undefined : buckets.get(key)
    const spansPages = !!bucket && bucket.some(m => m.page !== bucket[0].page)
    if (!key || !bucket || !spansPages) { out.push(build([h])); return }
    if (emitted.has(key)) return
    emitted.add(key)
    out.push(build(bucket))
  })
  return out
}

/**
 * What every record of a group must carry after a group-wide write that is NOT itself a
 * colour / style / line-break choice (a note edit, the line-break toggle).
 *
 * Those writes stamp ONE shared `updated_at` on all members. The group takes its colour,
 * style and line-break choice from the most recently edited member, so once the stamps
 * tie the signal is gone and the lowest page wins — a pair an older build had edited
 * unevenly would visibly flip colour on an unrelated note save. Writing the values the
 * row already shows makes the members agree instead, so nothing changes on screen.
 */
export function groupAppearance(g: HighlightGroup): Pick<Highlight, 'color' | 'style' | 'keep_line_breaks'> {
  return {
    color: g.color,
    ...(g.style ? { style: g.style } : {}),
    keep_line_breaks: g.keepLineBreaks ? true : undefined,
  }
}

/** Map every member id to its group, so a click on either half resolves the whole. */
export function indexGroups(groups: readonly HighlightGroup[]): Map<string, HighlightGroup> {
  const byId = new Map<string, HighlightGroup>()
  for (const g of groups) for (const id of g.ids) byId.set(id, g)
  return byId
}
