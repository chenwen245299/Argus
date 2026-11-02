import { isTauri } from '@tauri-apps/api/core'
import { Image as TauriImage } from '@tauri-apps/api/image'
import { writeImage } from '@tauri-apps/plugin-clipboard-manager'

function imageCanvas(src: string): Promise<HTMLCanvasElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.crossOrigin = 'anonymous'
    image.onload = () => {
      try {
        if (!image.naturalWidth || !image.naturalHeight) throw new Error('Empty image')
        const canvas = document.createElement('canvas')
        canvas.width = image.naturalWidth
        canvas.height = image.naturalHeight
        const context = canvas.getContext('2d')
        if (!context) throw new Error('Image canvas is unavailable')
        context.drawImage(image, 0, 0)
        resolve(canvas)
      } catch (error) { reject(error) }
    }
    image.onerror = () => reject(new Error('Could not load the image'))
    image.src = src
  })
}

/** Copy original pixels, independently of the preview's CSS size. */
export async function copyImageToClipboard(src: string): Promise<void> {
  if (isTauri()) {
    const canvas = await imageCanvas(src)
    const rgba = canvas.getContext('2d')!.getImageData(0, 0, canvas.width, canvas.height).data
    // RGBA avoids relying on Tauri's optional PNG decoder feature. The native
    // clipboard also works after asynchronous image loading in macOS WebKit.
    const image = await TauriImage.new(new Uint8Array(rgba.buffer, rgba.byteOffset, rgba.byteLength), canvas.width, canvas.height)
    try { await writeImage(image) }
    finally { await image.close().catch(() => {}) }
    return
  }
  // Keep the write in the click's user gesture, including in Safari: give the
  // ClipboardItem a pending PNG rather than awaiting image conversion first.
  if (typeof ClipboardItem === 'undefined' || !navigator.clipboard?.write) {
    throw new Error('Image clipboard is unavailable')
  }
  const png = imageCanvas(src).then(canvas => new Promise<Blob>((resolve, reject) => {
    canvas.toBlob(blob => blob ? resolve(blob) : reject(new Error('Could not encode the image')), 'image/png')
  }))
  await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })])
}
