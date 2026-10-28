// Run: node --test scripts/test-readable-stream-iterator.mjs. No library, browser or network.
// WebKit through Safari 26.5 cannot `for await` a ReadableStream, and pdf.js's getTextContent()
// does exactly that: pages drew but had no text layer. Node has the native iterator, so each
// test strips it from a stream class (or from ReadableStream itself) to stand in for that WebKit.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { pathToFileURL } from 'node:url'
import ts from 'typescript'

const source = await readFile(new URL('../src/utils/readableStreamAsyncIterator.ts', import.meta.url), 'utf8')
const { installReadableStreamAsyncIterator } =
  await import('data:text/javascript;base64,' + Buffer.from(ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
  }).outputText).toString('base64'))

/** A ReadableStream subclass without async iteration, as old WebKit has it. */
function streamWithoutIteration() {
  class OldWebKitStream extends ReadableStream {}
  for (const key of [Symbol.asyncIterator, 'values']) {
    Object.defineProperty(OldWebKitStream.prototype, key, { value: undefined, writable: true, configurable: true })
  }
  return OldWebKitStream
}

function source3(log = []) {
  let n = 0
  return {
    pull(controller) { n < 3 ? controller.enqueue(++n) : controller.close() },
    cancel(reason) { log.push(['cancel', reason]) },
  }
}

test('leaves a native implementation alone', () => {
  assert.equal(typeof ReadableStream.prototype[Symbol.asyncIterator], 'function')
  const before = ReadableStream.prototype[Symbol.asyncIterator]
  assert.equal(installReadableStreamAsyncIterator(ReadableStream), false)
  assert.equal(ReadableStream.prototype[Symbol.asyncIterator], before)
  assert.equal(installReadableStreamAsyncIterator(undefined), false)
})

test('for await reads every chunk in order, then unlocks the stream', async () => {
  const Stream = streamWithoutIteration()
  await assert.rejects(async () => { for await (const _ of new Stream(source3())) { /* */ } }, TypeError)
  assert.equal(installReadableStreamAsyncIterator(Stream), true)
  const stream = new Stream(source3())
  const seen = []
  for await (const value of stream) seen.push(value)
  assert.deepEqual(seen, [1, 2, 3])
  assert.equal(stream.locked, false)
  assert.equal(Object.keys(Stream.prototype).includes('values'), false, 'not enumerable, like the native one')
})

test('leaving the loop early cancels the stream; preventCancel keeps it readable', async () => {
  const Stream = streamWithoutIteration()
  installReadableStreamAsyncIterator(Stream)

  const log = []
  const stream = new Stream(source3(log))
  for await (const value of stream) { if (value === 1) break }
  assert.equal(stream.locked, false)
  assert.deepEqual(log, [['cancel', undefined]])

  const kept = []
  const stream2 = new Stream(source3(kept))
  for await (const value of stream2.values({ preventCancel: true })) { if (value === 1) break }
  assert.equal(stream2.locked, false)
  assert.deepEqual(kept, [])
  const rest = []
  for await (const value of stream2) rest.push(value)
  assert.deepEqual(rest, [2, 3])
})

test('a failing stream rejects the loop and releases its lock', async () => {
  const Stream = streamWithoutIteration()
  installReadableStreamAsyncIterator(Stream)
  const boom = new Error('boom')
  const stream = new Stream({ pull(controller) { controller.error(boom) } })
  await assert.rejects(async () => { for await (const _ of stream) { /* */ } }, boom)
  assert.equal(stream.locked, false)
})

test('a locked stream cannot be iterated', () => {
  const Stream = streamWithoutIteration()
  installReadableStreamAsyncIterator(Stream)
  const stream = new Stream(source3())
  stream.getReader()
  assert.throws(() => stream[Symbol.asyncIterator](), TypeError)
})

/** A one-page PDF reading "Hello Argus", built by hand (byte offsets computed). */
function tinyPdf() {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ]
  const content = 'BT /F1 18 Tf 40 100 Td (Hello Argus) Tj ET'
  objects.push(`<< /Length ${content.length} >>\nstream\n${content}\nendstream`)
  let pdf = '%PDF-1.4\n'
  const offsets = []
  objects.forEach((body, i) => { offsets.push(pdf.length); pdf += `${i + 1} 0 obj\n${body}\nendobj\n` })
  const xref = pdf.length
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`
  for (const off of offsets) pdf += `${String(off).padStart(10, '0')} 00000 n \n`
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
  return new TextEncoder().encode(pdf)
}

test("pdf.js's getTextContent() needs it, and works with it", async () => {
  const require = createRequire(import.meta.url)
  const pdfjsLib = await import(pathToFileURL(require.resolve('pdfjs-dist/legacy/build/pdf.mjs')).href)
  pdfjsLib.GlobalWorkerOptions.workerSrc =
    pathToFileURL(require.resolve('pdfjs-dist/legacy/build/pdf.worker.mjs')).href
  const proto = ReadableStream.prototype
  const native = { iterator: proto[Symbol.asyncIterator], values: proto.values }
  const doc = await pdfjsLib.getDocument({
    data: tinyPdf(), isEvalSupported: false, useSystemFonts: false,
    standardFontDataUrl: require.resolve('pdfjs-dist/package.json').replace(/package\.json$/, 'standard_fonts/'),
  }).promise
  try {
    const page = await doc.getPage(1)
    // As old WebKit: the stream exists, the iterator does not.
    Object.defineProperty(proto, Symbol.asyncIterator, { value: undefined, writable: true, configurable: true })
    Object.defineProperty(proto, 'values', { value: undefined, writable: true, configurable: true })
    await assert.rejects(page.getTextContent(), TypeError)

    assert.equal(installReadableStreamAsyncIterator(ReadableStream), true)
    const content = await page.getTextContent()
    assert.equal(content.items.map(item => item.str).join(''), 'Hello Argus')
  } finally {
    Object.defineProperty(proto, Symbol.asyncIterator, { value: native.iterator, writable: true, configurable: true })
    Object.defineProperty(proto, 'values', { value: native.values, writable: true, configurable: true })
    await doc.destroy()
  }
})
