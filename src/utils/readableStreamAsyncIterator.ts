/**
 * `for await (const chunk of readableStream)` for WebKits that lack it.
 *
 * pdf.js 5's `page.getTextContent()` reads its text stream with `for await`, and
 * neither its modern nor its legacy build polyfills stream iteration. WebKit
 * shipped it only after Safari 26.5 (webkit.org/b/194379), and a Tauri window is
 * the system WebKit, so on such a Mac `getTextContent()` throws: every page still
 * draws but gets no text layer at all — nothing can be selected, the page behaves
 * like a picture. Section detection and the saved full text fail the same way.
 *
 * Follows the Streams spec's ReadableStream async iterator: `values()` locks the
 * stream, a finished or failed read releases it, and leaving the loop early
 * cancels the stream unless `preventCancel` is set. Native support is left alone.
 */
export function installReadableStreamAsyncIterator(
  Stream: { prototype: object } | undefined = (globalThis as { ReadableStream?: { prototype: object } }).ReadableStream,
): boolean {
  if (!Stream) return false
  const proto = Stream.prototype as Record<PropertyKey, unknown>
  if (typeof proto[Symbol.asyncIterator] === 'function') return false

  type Reader = {
    read(): Promise<{ done: boolean; value?: unknown }>
    cancel(reason?: unknown): Promise<void>
    releaseLock(): void
  }

  function values(this: { getReader(): Reader }, options?: { preventCancel?: boolean }) {
    const preventCancel = !!options?.preventCancel
    const reader = this.getReader()
    let finished = false
    const finish = () => {
      if (finished) return
      finished = true
      reader.releaseLock()
    }
    return {
      async next() {
        if (finished) return { done: true, value: undefined }
        let result: { done: boolean; value?: unknown }
        try {
          result = await reader.read()
        } catch (error) {
          finish()
          throw error
        }
        if (result.done) {
          finish()
          return { done: true, value: undefined }
        }
        return { done: false, value: result.value }
      },
      async return(value?: unknown) {
        if (!finished) {
          finished = true
          if (preventCancel) {
            reader.releaseLock()
          } else {
            const cancelled = reader.cancel(value)
            reader.releaseLock()
            await cancelled
          }
        }
        return { done: true, value }
      },
      [Symbol.asyncIterator]() { return this },
    }
  }

  const define = (key: PropertyKey, fn: unknown) =>
    Object.defineProperty(proto, key, { value: fn, writable: true, configurable: true, enumerable: false })
  if (typeof proto.values !== 'function') define('values', values)
  define(Symbol.asyncIterator, proto.values)
  return true
}
