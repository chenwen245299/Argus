export async function copyPngBlobToClipboard(pngBlob: Blob): Promise<void> {
  const item = new ClipboardItem({ 'image/png': pngBlob })
  await navigator.clipboard.write([item])
}
