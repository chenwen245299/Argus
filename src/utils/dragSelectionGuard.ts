// Dragging a resize grabber must not double as a text drag.
//
// The app root sets `user-select: none`, but anything that opts back in with
// `.selectable-text` (the highlight-note body, for one) is still selectable —
// and that's exactly what sits under the pointer while a popup is being resized.
// The browser then anchors a selection there and extends it as the drag moves.
//
// Suppressing it with `preventDefault()` on pointerdown is the obvious fix and
// the wrong one: it also cancels the compatibility mouse events, and the popup's
// dismiss logic keys off mousedown/mouseup. So instead a body class turns the
// opt-in off for the duration of the drag, leaving the event flow untouched.

const DRAGGING_CLASS = 'argus-dragging'

export function beginDragSelectionGuard() {
  document.body.classList.add(DRAGGING_CLASS)
  // A selection may already exist from an earlier click; collapse it so the drag
  // doesn't visually extend it.
  const selection = window.getSelection()
  if (selection && !selection.isCollapsed) selection.removeAllRanges()

  // Safety net. Pointer capture makes a missing pointerup unlikely, but if one
  // ever went astray the class would stick and NOTHING in the app would be
  // selectable again until reload — a far worse bug than the one being fixed.
  window.addEventListener('pointerup', endDragSelectionGuard, { once: true })
  window.addEventListener('pointercancel', endDragSelectionGuard, { once: true })
  window.addEventListener('blur', endDragSelectionGuard, { once: true })
}

export function endDragSelectionGuard() {
  document.body.classList.remove(DRAGGING_CLASS)
  window.removeEventListener('pointerup', endDragSelectionGuard)
  window.removeEventListener('pointercancel', endDragSelectionGuard)
  window.removeEventListener('blur', endDragSelectionGuard)
}
