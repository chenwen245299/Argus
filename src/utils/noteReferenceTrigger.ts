export const NOTE_REFERENCE_EDITORS = '.vditor-ir .vditor-reset[contenteditable="true"],textarea[data-note-references]'
export interface ReferenceTrigger {
  editor: HTMLElement | HTMLTextAreaElement
  query: string
  token: string
  kind: 'command' | 'reference'
  source: string
  range?: Range
  start?: number
  end?: number
  x: number
  y: number
}
function matchTrigger(text: string): { token: string; query: string; kind: ReferenceTrigger['kind'] } | null {
  const at = text.lastIndexOf('/ref')
  const boundary = (offset: number) => /(?:^|[\s（(，,：:\p{Script=Han}])$/u.test(text.slice(0, offset))
  if (at >= 0 && boundary(at)) {
    const query = text.slice(at + 4)
    if ((!query || /^\s/.test(query)) && query.length <= 160 && !query.includes('\n')) {
      return { token: text.slice(at), query: query.trimStart(), kind: 'reference' }
    }
  }
  const slash = text.lastIndexOf('/')
  if (slash < 0 || !boundary(slash) || !['', 'r', 're'].includes(text.slice(slash + 1))) return null
  return { token: text.slice(slash), query: '', kind: 'command' }
}

export function getReferenceTrigger(): ReferenceTrigger | null {
  const active = document.activeElement
  if (active instanceof HTMLTextAreaElement && active.matches(NOTE_REFERENCE_EDITORS)) {
    if (active.disabled || active.readOnly || active.selectionStart !== active.selectionEnd) return null
    const end = active.selectionEnd, match = matchTrigger(active.value.slice(0, end))
    if (!match) return null
    const token = match.token
    // Match the textarea's wrapping and font without changing its content/undo.
    const mirror = document.createElement('div'), style = getComputedStyle(active), box = active.getBoundingClientRect()
    for (const name of ['font', 'letter-spacing', 'line-height', 'padding', 'border', 'box-sizing', 'tab-size']) mirror.style.setProperty(name, style.getPropertyValue(name))
    Object.assign(mirror.style, { position: 'fixed', visibility: 'hidden', whiteSpace: 'pre-wrap', overflowWrap: 'break-word', width: `${box.width}px`, left: `${box.left}px`, top: `${box.top}px` })
    mirror.textContent = active.value.slice(0, end)
    const caret = document.createElement('span'); caret.textContent = '\u200b'; mirror.append(caret); document.body.append(mirror)
    const rect = caret.getBoundingClientRect(); mirror.remove()
    return { editor: active, token, query: match.query, kind: match.kind, source: active.value, start: end - token.length, end, x: rect.left - active.scrollLeft, y: rect.bottom - active.scrollTop }
  }
  const selection = window.getSelection()
  if (!selection?.rangeCount || !selection.isCollapsed) return null
  const caret = selection.getRangeAt(0)
  const parent = caret.startContainer.nodeType === Node.ELEMENT_NODE ? caret.startContainer as Element : caret.startContainer.parentElement
  const editor = parent?.closest<HTMLElement>(NOTE_REFERENCE_EDITORS)
  if (!editor || !editor.contains(active) && active !== editor) return null
  if (parent?.closest('code,[data-type="code"],[data-type="code-block"],[data-type="inline-math"],[data-type="math-block"],[data-type="html-block"],[data-type="a"]')) return null
  const block = parent?.closest('p,li,h1,h2,h3,h4,h5,h6,td,th') ?? editor
  if (!editor.contains(block)) return null
  const prefix = caret.cloneRange(); prefix.selectNodeContents(block); prefix.setEnd(caret.startContainer, caret.startOffset)
  const text = prefix.toString(), match = matchTrigger(text)
  if (!match) return null
  const token = match.token, start = text.length - token.length
  const walker = document.createTreeWalker(block, NodeFilter.SHOW_TEXT)
  let offset = 0, node: Node | null
  while ((node = walker.nextNode())) {
    const length = node.textContent?.length ?? 0
    if (offset + length >= start) { prefix.setStart(node, start - offset); break }
    offset += length
  }
  if (!node) return null
  const rects = Array.from(prefix.getClientRects())
  const rect = caret.getClientRects()[0] ?? rects[rects.length - 1] ?? block.getBoundingClientRect()
  return { editor, query: match.query, kind: match.kind, source: editor.textContent ?? '', token, range: prefix, x: rect.left, y: rect.bottom }
}

/** The popup owns focus while searching; its original insertion anchor stays frozen. */
export function referenceTriggerValid(trigger: ReferenceTrigger): boolean {
  const editor = trigger.editor
  if (!editor.isConnected || !editor.matches(NOTE_REFERENCE_EDITORS)) return false
  if (editor instanceof HTMLTextAreaElement) {
    return !editor.disabled && !editor.readOnly && editor.value === trigger.source
      && editor.value.slice(trigger.start, trigger.end) === trigger.token
  }
  const range = trigger.range
  return editor.isContentEditable && editor.textContent === trigger.source && !!range
    && editor.contains(range.startContainer) && editor.contains(range.endContainer) && range.toString() === trigger.token
}

export function restoreReferenceCaret(trigger: ReferenceTrigger) {
  const editor = trigger.editor
  if (!editor.isConnected) return
  editor.focus({ preventScroll: true })
  if (editor instanceof HTMLTextAreaElement) {
    const end = Math.min(trigger.end ?? editor.value.length, editor.value.length)
    editor.setSelectionRange(end, end)
  } else if (trigger.range && editor.contains(trigger.range.endContainer)) {
    const caret = trigger.range.cloneRange(); caret.collapse(false)
    const selection = window.getSelection(); selection?.removeAllRanges(); selection?.addRange(caret)
  }
}

/** Use the native editing command so Vditor and textarea undo/autosave stay intact. */
export function insertReference(trigger: ReferenceTrigger, markdown: string) {
  const editor = trigger.editor
  editor.focus({ preventScroll: true })
  if (editor instanceof HTMLTextAreaElement) editor.setSelectionRange(trigger.start!, trigger.end!)
  else if (trigger.range) {
    const selection = window.getSelection(); selection?.removeAllRanges(); selection?.addRange(trigger.range)
  }
  const inserted = document.execCommand('insertText', false, markdown)
  if (!inserted && editor instanceof HTMLTextAreaElement) {
    editor.setRangeText(markdown, trigger.start!, trigger.end!, 'end')
    editor.dispatchEvent(new Event('input', { bubbles: true }))
  }
  return inserted || editor instanceof HTMLTextAreaElement
}
