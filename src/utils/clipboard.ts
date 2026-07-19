import { writeText } from '@tauri-apps/plugin-clipboard-manager'

export async function copyPngBlobToClipboard(pngBlob: Blob): Promise<void> {
  const item = new ClipboardItem({ 'image/png': pngBlob })
  await navigator.clipboard.write([item])
}

/** Copy plain text to the system clipboard through the Tauri backend. Unlike the
 *  web Clipboard API, this has no transient-user-gesture or window-focus timing
 *  constraints, so it stays reliable even when called after an `await` (e.g. once
 *  a BibTeX entry has been fetched). Falls back to the web API if the plugin is
 *  unavailable. Returns whether the copy succeeded. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await writeText(text)
    return true
  } catch {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      return false
    }
  }
}
