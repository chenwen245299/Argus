import { invoke } from '@tauri-apps/api/core'
import { svgPreviewUrl } from './svgPreview'

// Images pasted into a note are written to `<paper>/notes/assets/` and the
// markdown stores the RELATIVE path (`assets/x.png`), which is what keeps a
// paper folder portable.
//
// A webview can't load a relative path like that — it would resolve against the
// app origin, not the library on disk — so the bytes are read over IPC once and
// handed to the editor as a blob URL. The swap happens at the disk boundary:
//
//   disk ──toDisplayMarkdown──▶ editor ──toStoredMarkdown──▶ disk
//
// Nothing but `assets/...` is ever written to the note file, and no image bytes
// ever enter the markdown. Inlining them as a `data:` URI instead is what used
// to wedge the editor: a screenshot becomes multiple MB of base64 on a single
// line, re-rendered on every keystroke and rewritten to disk on every autosave.

/** `assets/<name>` ⇄ `blob:` URL, for this webview. */
const blobByPath = new Map<string, string>()
const pathByBlob = new Map<string, string>()

/** Both `![alt](path)` and a raw `<img src="path">`. */
const MD_IMAGE_RE = /(!\[[^\]]*\]\()([^)\s]+)(\))/g
const HTML_IMAGE_RE = /(<img\b[^>]*?\bsrc=")([^"]+)(")/g

const ASSET_PREFIX = 'assets/'

/** Validate actual SVG XML, then share the isolated SVG preview sanitizer. */
function sanitizedSvgBytes(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  const source = new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  const doc = new DOMParser().parseFromString(source, 'image/svg+xml')
  if (doc.documentElement.localName !== 'svg' || doc.querySelector('parsererror')) {
    throw new Error('Invalid SVG image')
  }
  // Serializing just the root drops XML declarations, comments and doctypes.
  const url = svgPreviewUrl(new XMLSerializer().serializeToString(doc.documentElement))
  if (!url) throw new Error('Invalid SVG image')
  return new TextEncoder().encode(decodeURIComponent(url.slice(url.indexOf(',') + 1)))
}

function assetKey(slug: string, relPath: string) {
  return `${slug}\u0000${relPath}`
}

/**
 * Resolve one `assets/<name>` to a blob URL, reading it over IPC the first time.
 * Returns null when the file is missing, leaving the original path in place.
 */
async function assetToBlobUrl(slug: string, relPath: string): Promise<string | null> {
  const key = assetKey(slug, relPath)
  const cached = blobByPath.get(key)
  if (cached) return cached

  const name = relPath.slice(ASSET_PREFIX.length)
  try {
    const b64 = await invoke<string>('read_note_asset', { slug, name })
    let bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0))
    const ext = name.split('.').pop()?.toLowerCase() ?? 'png'
    const mime = ext === 'jpg' || ext === 'jpeg' ? 'image/jpeg' : ext === 'svg' ? 'image/svg+xml' : `image/${ext}`
    if (ext === 'svg') bytes = sanitizedSvgBytes(bytes)
    const url = URL.createObjectURL(new Blob([bytes], { type: mime }))
    blobByPath.set(key, url)
    pathByBlob.set(url, relPath)
    return url
  } catch (e) {
    console.error('[noteAssets] failed to read', relPath, e)
    return null
  }
}

/** Rewrite stored markdown into something the editor can actually render. */
export async function toDisplayMarkdown(slug: string, markdown: string): Promise<string> {
  if (!markdown.includes(ASSET_PREFIX)) return markdown

  // Resolve every distinct asset once, then substitute — the replace itself has
  // to stay synchronous.
  const paths = new Set<string>()
  for (const re of [MD_IMAGE_RE, HTML_IMAGE_RE]) {
    re.lastIndex = 0
    for (const m of markdown.matchAll(re)) {
      if (m[2].startsWith(ASSET_PREFIX)) paths.add(m[2])
    }
  }
  if (!paths.size) return markdown

  const resolved = new Map<string, string>()
  await Promise.all([...paths].map(async (p) => {
    const url = await assetToBlobUrl(slug, p)
    if (url) resolved.set(p, url)
  }))

  const swap = (_m: string, pre: string, src: string, post: string) =>
    `${pre}${resolved.get(src) ?? src}${post}`
  return markdown.replace(MD_IMAGE_RE, swap).replace(HTML_IMAGE_RE, swap)
}

/**
 * Rewrite editor markdown back to what belongs on disk. A `blob:` URL we can't
 * map is dropped rather than persisted: it is dead the moment the window closes,
 * so writing it would bake a permanently broken link into the user's note.
 */
export function toStoredMarkdown(markdown: string): string {
  if (!markdown.includes('blob:')) return markdown
  const swap = (_m: string, pre: string, src: string, post: string) => {
    if (!src.startsWith('blob:')) return `${pre}${src}${post}`
    const relPath = pathByBlob.get(src)
    if (relPath) return `${pre}${relPath}${post}`
    console.error('[noteAssets] unmapped blob URL dropped from note')
    return ''
  }
  return markdown.replace(MD_IMAGE_RE, swap).replace(HTML_IMAGE_RE, swap)
}

/** Extension for a pasted file, from its MIME type. */
function extFor(file: File): string | null {
  const map: Record<string, string> = {
    'image/png': 'png',
    'image/jpeg': 'jpg',
    'image/gif': 'gif',
    'image/webp': 'webp',
    'image/svg+xml': 'svg',
  }
  const mime = file.type.toLowerCase().split(';')[0].trim()
  // Some drag sources omit MIME; only recognize SVG by name in that case.
  return map[mime] ?? (!mime && /\.svg$/i.test(file.name) ? 'svg' : null)
}

/**
 * Persist a pasted image and return the markdown to insert (`![](blob:…)`, which
 * `toStoredMarkdown` turns back into the relative path on save).
 */
export async function savePastedImage(slug: string, file: File, expectedRoot?: string | null): Promise<string | null> {
  const ext = extFor(file)
  if (!ext) return null   // not an image type we store

  if (!file.size || file.size > 20 * 1024 * 1024) throw new Error('Image is empty or exceeds 20 MB')
  const buffer = await file.arrayBuffer()
  const bytes = ext === 'svg' ? sanitizedSvgBytes(new Uint8Array(buffer)) : new Uint8Array(buffer)
  // Chunked so a large image can't blow the argument limit of String.fromCharCode.
  let binary = ''
  const CHUNK = 0x8000
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK))
  }

  const relPath = await invoke<string>('save_note_asset', {
    slug,
    ext,
    data: btoa(binary),
    expectedRoot,
  })
  const url = URL.createObjectURL(new Blob([bytes], { type: ext === 'svg' ? 'image/svg+xml' : file.type }))
  blobByPath.set(assetKey(slug, relPath), url)
  pathByBlob.set(url, relPath)
  return url
}
