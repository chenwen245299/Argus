import { ref } from 'vue'

// Shared geometry for the highlight-note ("批注") popup used by both readers.
// The popup is a resizable window: the user drags its bottom-right grabber in
// either direction and the size sticks across highlights, tabs and restarts.
// Living here rather than inside a viewer keeps PdfViewer and EbookViewer in
// sync without coupling their rendering paths.

const SIZE_KEY = 'argus:hl-note-popup-size'

export const NOTE_POPUP_MIN_W = 200
export const NOTE_POPUP_MIN_H = 90

export type NotePopupSize = { w: number; h: number }

const DEFAULT_SIZE: NotePopupSize = { w: 290, h: 130 }

function loadSize(): NotePopupSize {
  try {
    const raw = localStorage.getItem(SIZE_KEY)
    if (!raw) return { ...DEFAULT_SIZE }
    const parsed = JSON.parse(raw) as Partial<NotePopupSize> | null
    const w = Number(parsed?.w)
    const h = Number(parsed?.h)
    if (!Number.isFinite(w) || !Number.isFinite(h)) return { ...DEFAULT_SIZE }
    return { w: Math.max(NOTE_POPUP_MIN_W, w), h: Math.max(NOTE_POPUP_MIN_H, h) }
  } catch {
    return { ...DEFAULT_SIZE }
  }
}

export const notePopupSize = ref<NotePopupSize>(loadSize())

let persistTimer: number | undefined
function persistSize() {
  window.clearTimeout(persistTimer)
  persistTimer = window.setTimeout(() => {
    try {
      localStorage.setItem(SIZE_KEY, JSON.stringify(notePopupSize.value))
    } catch { /* private mode / quota — size still applies for this session */ }
  }, 200)
}

export function setNotePopupSize(w: number, h: number) {
  const next = { w: Math.max(NOTE_POPUP_MIN_W, Math.round(w)), h: Math.max(NOTE_POPUP_MIN_H, Math.round(h)) }
  if (next.w === notePopupSize.value.w && next.h === notePopupSize.value.h) return
  notePopupSize.value = next
  persistSize()
}

// Width/height are pinned by the inline style below, so the box can only change
// when the user drags the CSS resize grabber — no need to filter content resizes.
export function observeNotePopupResize(el: HTMLElement): () => void {
  if (typeof ResizeObserver === 'undefined') return () => {}
  const ro = new ResizeObserver(() => setNotePopupSize(el.offsetWidth, el.offsetHeight))
  ro.observe(el)
  return () => ro.disconnect()
}

// Anchor the popup at the click point, pulled back inside the viewport — a window
// the user has grown can easily reach past the right/bottom edge. Clamp ONCE when
// the popup opens: folding this into the reactive style would re-run it on every
// resize tick, sliding the popup sideways while the user drags the grabber.
export function clampNotePopupPos(x: number, y: number): { x: number; y: number } {
  const { w, h } = notePopupSize.value
  const maxLeft = Math.max(8, window.innerWidth - w - 8)
  const maxTop = Math.max(8, window.innerHeight - h - 8)
  return {
    x: Math.min(Math.max(8, x), maxLeft),
    y: Math.min(Math.max(8, y), maxTop),
  }
}

export function notePopupStyle(x: number, y: number): Record<string, string> {
  const { w, h } = notePopupSize.value
  return { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` }
}
