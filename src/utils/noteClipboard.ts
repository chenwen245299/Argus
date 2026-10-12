/** Install after Vditor's own handlers so its Markdown/TeX conversion and cut
 *  behavior stay intact. Vditor writes an empty HTML flavor alongside the text;
 *  native rich-text applications can prefer that flavor and paste nothing. */
export function installNoteClipboard(host: HTMLElement): () => void {
  const editors = host.querySelectorAll<HTMLElement>(
    '.vditor-ir .vditor-reset, .vditor-wysiwyg .vditor-reset',
  )
  const repair = (event: ClipboardEvent) => {
    const data = event.clipboardData
    if (!event.defaultPrevented || !data?.getData('text/plain')) return
    if (data.types.includes('text/html') && data.getData('text/html') === '') {
      data.clearData('text/html')
    }
  }
  // Vditor stops propagation, so a wrapper/document listener never sees these
  // events. Later listeners on the same editor element still run.
  for (const editor of editors) {
    editor.addEventListener('copy', repair)
    editor.addEventListener('cut', repair)
  }
  return () => {
    for (const editor of editors) {
      editor.removeEventListener('copy', repair)
      editor.removeEventListener('cut', repair)
    }
  }
}
