// Promise.withResolvers polyfill — must run before PDF.js worker code.
// Worker threads are isolated from the main thread, so the polyfill in main.ts
// doesn't reach here. Ventura's WKWebView (WebKit < 17.4) lacks this API.
if (typeof (Promise as any).withResolvers === 'undefined') {
  ;(Promise as any).withResolvers = function <T>() {
    let resolve!: (value: T | PromiseLike<T>) => void
    let reject!:  (reason?: unknown) => void
    const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej })
    return { promise, resolve, reject }
  }
}

// Dynamic import (not static) ensures the polyfill above runs first.
// Messages from the main thread are queued by the browser until this resolves.
// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore — no type declarations for the worker entry point
import('pdfjs-dist/build/pdf.worker.min.mjs')
