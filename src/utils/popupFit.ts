/**
 * Keeps a `position: fixed` popup inside the window.
 *
 * The selection toolbar is anchored to selected text and its width depends on the UI language
 * and on which buttons are present (read-aloud, the page-furniture toggle), so a fixed `left`
 * can push the right-hand buttons past the window edge. The viewers measure the laid-out popup
 * and shift it by the amount this returns. It only ever returns a shift for the popup as it is
 * RIGHT NOW: callers apply it when the popup opens or changes size, never while the page
 * scrolls, because the popup follows its anchor then and clamping would re-detach it.
 */
export interface BoxRect { left: number; top: number; right: number; bottom: number }

/** Anchor below the last visible selected text run, independent of the mouse release. */
export function selectionPopupAnchor(
  rects: readonly BoxRect[],
  win: { width: number; height: number },
): { x: number; y: number } | null {
  for (let i = rects.length - 1; i >= 0; i--) {
    const r = rects[i]
    if (r.right <= r.left || r.bottom <= r.top || r.bottom <= 0 || r.top >= win.height
      || r.right <= 0 || r.left >= win.width) continue
    return { x: Math.max(0, r.left), y: Math.min(win.height, r.bottom) + 12 }
  }
  return null
}

export interface FitOptions {
  /** Smallest gap kept between the popup and each window edge (px). */
  margin?: number
  /**
   * When the popup does not fit below its anchor, put it ABOVE the anchor instead of sliding it
   * up over the anchor: the distance between the anchor and the popup's near edge, which the
   * popup keeps on the other side. Omit to simply slide it up.
   */
  flipGap?: number
}

export function popupShift(
  box: BoxRect,
  win: { width: number; height: number },
  opts: FitOptions = {},
): { dx: number; dy: number } {
  const m = opts.margin ?? 8
  const w = box.right - box.left
  const h = box.bottom - box.top

  // A popup wider than the window cannot fit; it is pinned to the left margin and the CSS
  // max-width + wrap keep that case rare. Math.max(m, ...) is what makes the left edge win.
  const left = Math.max(m, Math.min(box.left, win.width - m - w))

  let top = box.top
  if (box.bottom > win.height - m && opts.flipGap !== undefined) {
    const above = box.top - 2 * opts.flipGap - h
    top = above >= m ? above : Math.max(m, win.height - m - h)
  } else {
    top = Math.max(m, Math.min(box.top, win.height - m - h))
  }
  return { dx: left - box.left, dy: top - box.top }
}
