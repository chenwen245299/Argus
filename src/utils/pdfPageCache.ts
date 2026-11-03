// Lossless, device-local page bitmaps. PDF/OCR/highlight data stays in the library;
// this disposable cache only avoids decoding the same scanned page again.
const DB_NAME = 'argus-pdf-pages-v1'
const MAX_BYTES = 512 * 1024 * 1024
const MAX_ENTRIES = 400
const MAX_PAGE_BYTES = 32 * 1024 * 1024
const READ_TIMEOUT_MS = 120

interface CachedPage {
  key: string
  blob: Blob
  bytes: number
  used: number
}
interface Usage { id: string; bytes: number; count: number }
let database: Promise<IDBDatabase | null> | undefined
let available: boolean | null = null
let writes: Promise<void> = Promise.resolve()

function openDatabase(): Promise<IDBDatabase | null> {
  if (database) return database
  database = new Promise(resolve => {
    let settled = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const finish = (db: IDBDatabase | null) => {
      if (settled) { db?.close(); return }
      settled = true
      if (timer !== undefined) clearTimeout(timer)
      available = !!db
      resolve(db)
    }
    // A blocked database must not retain an unlimited queue of encoded pages.
    timer = setTimeout(() => finish(null), 1000)
    try {
      const request = indexedDB.open(DB_NAME, 1)
      request.onupgradeneeded = () => {
        request.result.createObjectStore('pages', { keyPath: 'key' }).createIndex('used', 'used')
        request.result.createObjectStore('usage', { keyPath: 'id' })
      }
      request.onsuccess = () => {
        const db = request.result
        db.onversionchange = () => { db.close(); database = undefined; available = null }
        finish(db)
      }
      request.onerror = () => finish(null)
      request.onblocked = () => finish(null)
    } catch { finish(null) }
  })
  return database
}

/** Hash the actual bytes before PDF.js transfers them to its worker. Same-name
 *  replacements and different libraries cannot accidentally reuse old pages. */
export async function pdfDocumentCacheKey(bytes: Uint8Array, renderer: string): Promise<string | null> {
  try {
    const hash = await crypto.subtle.digest('SHA-256', bytes as BufferSource)
    const hex = [...new Uint8Array(hash)].map(b => b.toString(16).padStart(2, '0')).join('')
    return `${renderer}:${hex}`
  } catch { return null } // Older/private WebViews still have the normal render path.
}

export function pdfPageCacheKey(document: string | null, page: number, width: number, height: number): string | null {
  return document ? `${document}:${page}:${width}x${height}` : null
}

/** Cache IO is optional and must never hold up a visible page on a blocked or
 *  unusually slow IndexedDB. Late reads create no object URLs or DOM nodes. */
export async function readPdfPageCache(key: string | null): Promise<Blob | null> {
  if (!key || available === false) return null
  let timer: ReturnType<typeof setTimeout> | undefined
  const read = (async () => {
    const db = await openDatabase()
    if (!db) return null
    return new Promise<Blob | null>(resolve => {
      try {
        const tx = db.transaction('pages', 'readwrite'), store = tx.objectStore('pages')
        const request = store.get(key)
        let blob: Blob | null = null
        request.onsuccess = () => {
          const entry = request.result as CachedPage | undefined
          if (entry?.blob instanceof Blob && entry.blob.type === 'image/png' && entry.blob.size > 0) {
            blob = entry.blob
            store.put({ ...entry, used: Date.now() })
          }
        }
        tx.oncomplete = () => resolve(blob)
        tx.onerror = tx.onabort = () => resolve(null)
      } catch { resolve(null) }
    })
  })().catch(() => null)
  try {
    return await Promise.race([read, new Promise<null>(resolve => { timer = setTimeout(() => resolve(null), READ_TIMEOUT_MS) })])
  } finally { if (timer !== undefined) clearTimeout(timer) }
}

function mutate(key: string, blob: Blob | null): Promise<void> {
  return (async () => {
    const db = await openDatabase()
    if (!db) return
    await new Promise<void>(resolve => {
      try {
        const tx = db.transaction(['pages', 'usage'], 'readwrite')
        const pages = tx.objectStore('pages'), totals = tx.objectStore('usage')
        const oldRequest = pages.get(key), usageRequest = totals.get('total')
        let pending = 2
        const update = () => {
          if (--pending) return
          const old = oldRequest.result as CachedPage | undefined
          const usage: Usage = usageRequest.result ?? { id: 'total', bytes: 0, count: 0 }
          if (old) { usage.bytes -= old.bytes; usage.count-- }
          if (blob) {
            pages.put({ key, blob, bytes: blob.size, used: Date.now() } satisfies CachedPage)
            usage.bytes += blob.size; usage.count++
          } else pages.delete(key)
          // Walk only as far as needed; never load the whole cache into JS.
          const cursorRequest = pages.index('used').openCursor()
          cursorRequest.onsuccess = () => {
            const cursor = cursorRequest.result
            if (cursor && (usage.bytes > MAX_BYTES || usage.count > MAX_ENTRIES)) {
              const entry = cursor.value as CachedPage
              if (entry.key !== key) {
                usage.bytes -= entry.bytes; usage.count--; cursor.delete()
              }
              cursor.continue()
            } else totals.put(usage)
          }
        }
        oldRequest.onsuccess = usageRequest.onsuccess = update
        tx.oncomplete = tx.onerror = tx.onabort = () => resolve()
      } catch { resolve() }
    })
  })().catch(() => {}) // Quota/private-mode failures cannot fail the reader.
}

export function writePdfPageCache(key: string | null, blob: Blob): Promise<void> {
  if (!key || available === false || blob.type !== 'image/png' || !blob.size || blob.size > MAX_PAGE_BYTES) return Promise.resolve()
  const operation = writes.then(() => mutate(key, blob))
  writes = operation.catch(() => {})
  return operation
}

export function deletePdfPageCache(key: string | null): Promise<void> {
  if (!key) return Promise.resolve()
  const operation = writes.then(() => mutate(key, null))
  writes = operation.catch(() => {})
  return operation
}

/** toBlob snapshots the finished canvas before eviction can zero its backing
 *  store. PNG encoding/storage stays outside the page's render/selection path. */
export function cachePdfCanvas(key: string | null, canvas: HTMLCanvasElement): void {
  if (!key || available === false || !canvas.width || !canvas.height) return
  try {
    canvas.toBlob(blob => { if (blob) void writePdfPageCache(key, blob) }, 'image/png')
  } catch { /* An unavailable encoder does not affect the displayed page. */ }
}
