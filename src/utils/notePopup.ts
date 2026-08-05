import { ref } from 'vue'

// Shared geometry for the highlight-note ("批注") popup used by both readers.
// The popup is a resizable window: the user drags its bottom-right grabber in
// either direction and that size sticks to THAT highlight across tabs and
// restarts. Sizes are per-highlight on purpose — one long note needing a tall
// box must not stretch every other note's popup — and a highlight the user
// never resized always opens at the default size.
// Living here rather than inside a viewer keeps PdfViewer and EbookViewer in
// sync without coupling their rendering paths.

const SIZES_KEY = 'argus:hl-note-popup-sizes'
/** Pre-per-highlight key, which held a single size for every popup. */
const LEGACY_SIZE_KEY = 'argus:hl-note-popup-size'

export const NOTE_POPUP_MIN_W = 200
export const NOTE_POPUP_MIN_H = 90

export type NotePopupSize = { w: number; h: number }

const DEFAULT_SIZE: NotePopupSize = { w: 290, h: 130 }

// Entries outlive the highlights they belong to (a note deleted in another
// window leaves one behind), so the map is capped and evicts oldest-first.
const MAX_ENTRIES = 500

function sanitize(value: unknown): NotePopupSize | null {
  const w = Number((value as Partial<NotePopupSize> | null)?.w)
  const h = Number((value as Partial<NotePopupSize> | null)?.h)
  if (!Number.isFinite(w) || !Number.isFinite(h)) return null
  return { w: Math.max(NOTE_POPUP_MIN_W, Math.round(w)), h: Math.max(NOTE_POPUP_MIN_H, Math.round(h)) }
}

function loadSizes(): Record<string, NotePopupSize> {
  // The old global size is deliberately NOT migrated: every highlight starts at
  // the default now, and inheriting one stretched box would recreate exactly
  // the behaviour this replaced.
  try { localStorage.removeItem(LEGACY_SIZE_KEY) } catch { /* ignore */ }
  try {
    const raw = localStorage.getItem(SIZES_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as Record<string, unknown> | null
    if (!parsed || typeof parsed !== 'object') return {}
    const out: Record<string, NotePopupSize> = {}
    for (const [id, value] of Object.entries(parsed)) {
      const size = sanitize(value)
      if (size) out[id] = size
    }
    return out
  } catch {
    return {}
  }
}

const notePopupSizes = ref<Record<string, NotePopupSize>>(loadSizes())

let persistTimer: number | undefined
function persistSizes() {
  window.clearTimeout(persistTimer)
  persistTimer = window.setTimeout(() => {
    try {
      localStorage.setItem(SIZES_KEY, JSON.stringify(notePopupSizes.value))
    } catch { /* private mode / quota — sizes still apply for this session */ }
  }, 200)
}

/** The size stored for a highlight, or the default when it was never resized. */
export function notePopupSizeFor(hlId: string | null | undefined): NotePopupSize {
  return (hlId ? notePopupSizes.value[hlId] : null) ?? DEFAULT_SIZE
}

export function setNotePopupSize(hlId: string, w: number, h: number) {
  const next = sanitize({ w, h })
  if (!next) return
  const current = notePopupSizeFor(hlId)
  // An untouched popup reports its default size on open; storing that would
  // fill the map with no-op entries, so only real changes are kept.
  if (next.w === current.w && next.h === current.h) return
  const map = { ...notePopupSizes.value, [hlId]: next }
  const ids = Object.keys(map)
  // String keys keep insertion order, so the oldest entries are at the front.
  for (const stale of ids.slice(0, Math.max(0, ids.length - MAX_ENTRIES))) delete map[stale]
  notePopupSizes.value = map
  persistSizes()
}

/** Drop a highlight's stored size when the highlight itself goes away. */
export function forgetNotePopupSize(hlId: string) {
  if (!(hlId in notePopupSizes.value)) return
  const map = { ...notePopupSizes.value }
  delete map[hlId]
  notePopupSizes.value = map
  persistSizes()
}

// Width/height are pinned by the inline style below, so the box can only change
// when the user drags the CSS resize grabber — no need to filter content resizes.
export function observeNotePopupResize(el: HTMLElement, hlId: string): () => void {
  if (typeof ResizeObserver === 'undefined') return () => {}
  const ro = new ResizeObserver(() => setNotePopupSize(hlId, el.offsetWidth, el.offsetHeight))
  ro.observe(el)
  return () => ro.disconnect()
}

// Anchor the popup at the click point, pulled back inside the viewport — a window
// the user has grown can easily reach past the right/bottom edge. Clamp ONCE when
// the popup opens: folding this into the reactive style would re-run it on every
// resize tick, sliding the popup sideways while the user drags the grabber.
export function clampNotePopupPos(x: number, y: number, hlId: string): { x: number; y: number } {
  const { w, h } = notePopupSizeFor(hlId)
  const maxLeft = Math.max(8, window.innerWidth - w - 8)
  const maxTop = Math.max(8, window.innerHeight - h - 8)
  return {
    x: Math.min(Math.max(8, x), maxLeft),
    y: Math.min(Math.max(8, y), maxTop),
  }
}

export function notePopupStyle(x: number, y: number, hlId: string): Record<string, string> {
  const { w, h } = notePopupSizeFor(hlId)
  return { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` }
}
