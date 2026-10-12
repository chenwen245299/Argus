// Run: node --test scripts/test-pdf-text-layer.mjs. No library or system fonts required.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

const source = await readFile(new URL('../src/utils/pdfTextLayer.ts', import.meta.url), 'utf8')
const { renderSelectableTextLayer, hasSelectableText, buildTextLayerFallback, TEXT_LAYER_TIMEOUT_MS } =
  await import('data:text/javascript;base64,' + Buffer.from(ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
  }).outputText).toString('base64'))

class Element {
  constructor(tag, doc) {
    this.tag = tag; this.ownerDocument = doc; this.children = []; this.attrs = {}; this.dataset = {}
    this.style = { setProperty(name, value) { this[name] = value } }
    this.textContent = ''; this.parent = null; this.rootConnected = false; this.rect = null
  }
  get isConnected() { return this.rootConnected || !!this.parent?.isConnected }
  get parentElement() { return this.parent }
  setAttribute(name, value) { this.attrs[name] = value }
  append(...nodes) {
    for (const node of nodes) {
      if (node.tag === '#fragment') this.append(...node.children)
      else { node.parent = this; this.children.push(node) }
    }
  }
  replaceChildren(...nodes) {
    this.children.forEach(child => { child.parent = null })
    this.children = []; this.append(...nodes)
  }
  replaceWith(next) {
    if (this.parent) {
      const parent = this.parent
      parent.children[parent.children.indexOf(this)] = next
      next.parent = parent; this.parent = null
    }
  }
  cloneNode() {
    const node = new Element(this.tag, this.ownerDocument)
    Object.assign(node.style, this.style); Object.assign(node.dataset, this.dataset)
    return node
  }
  querySelectorAll() {
    return this.children.flatMap(child => [
      ...(child.tag === 'span' && child.attrs.role === 'presentation' ? [child] : []),
      ...child.querySelectorAll(),
    ])
  }
  getBoundingClientRect() {
    if (this.rect) return this.rect
    if (this.tag === 'div') return { width: this.isConnected ? 600 : 0, height: 800 }
    const height = Number(/([\d.]+)px/.exec(this.style.fontSize ?? '')?.[1] ?? 12)
    return { width: this.textContent.length * height / 2, height }
  }
}

const doc = {
  createElement: tag => new Element(tag, doc),
  createDocumentFragment: () => new Element('#fragment', doc),
}
const content = {
  items: [
    { str: '正文 α 中文', fontName: 'body', dir: 'ltr', transform: [12, 0, 0, 12, 50, 700], width: 80, height: 12, hasEOL: true },
    { str: 'Another line', fontName: 'body', dir: 'ltr', transform: [12, 0, 0, 12, 50, 680], width: 72, height: 12 },
  ],
  styles: { body: { fontFamily: 'serif', ascent: 0.75, descent: -0.25 } },
}
const viewport = { width: 600, height: 800, scale: 1, transform: [1, 0, 0, -1, 0, 800] }
function fixture() {
  const page = doc.createElement('div'); page.rootConnected = true
  const container = doc.createElement('div'); page.append(container)
  let cancellations = 0
  const normal = (root, { zero = false, partial = false } = {}) => {
    for (const item of content.items.slice(0, partial ? 1 : 2)) {
      const span = doc.createElement('span'); span.setAttribute('role', 'presentation')
      span.textContent = item.str
      if (zero) span.rect = { width: 0, height: 0 }
      root.append(span)
    }
  }
  const options = { container, content, viewport, cancelled: () => false,
    create: root => ({ render: async () => normal(root), cancel: () => cancellations++ }) }
  return { page, container, normal, options, get cancellations() { return cancellations } }
}
const copiedText = container => container.querySelectorAll().map(span => span.textContent).join('\n')

test('a healthy text layer stays on the normal engine with its original DOM', async () => {
  const h = fixture()
  const result = await renderSelectableTextLayer(h.options)
  assert.equal(result, h.container)
  assert.equal(copiedText(result), '正文 α 中文\nAnother line')
  assert.equal(h.cancellations, 0)
  assert.equal(result.dataset.textLayerEngine, undefined)
})

test('a font-layout rejection recovers complete selectable text and retires partial work', async t => {
  t.mock.method(console, 'warn', () => {})
  const h = fixture()
  h.options.create = root => ({ render: async () => {
    h.normal(root, { partial: true }); throw new Error('Font measurement unavailable')
  }, cancel: () => {} })
  const result = await renderSelectableTextLayer(h.options)
  assert.notEqual(result, h.container)
  assert.equal(result.dataset.textLayerEngine, 'fallback')
  assert.equal(copiedText(result), '正文 α 中文\nAnother line')
  assert.equal(h.container.isConnected, false)
  // Even an old asynchronous task cannot duplicate text in the visible layer.
  h.normal(h.container)
  assert.equal(result.querySelectorAll().length, 2)
  assert.equal(h.page.children.length, 1)
})

test('zero-sized spans recover even when the original render promise resolved', async () => {
  const h = fixture()
  h.options.create = root => ({ render: async () => h.normal(root, { zero: true }), cancel: () => {} })
  const result = await renderSelectableTextLayer(h.options)
  assert.equal(result.dataset.textLayerEngine, 'fallback')
  assert.ok(hasSelectableText(result, content))
})

test('a working header cannot hide missing body-text boxes in another font', async () => {
  const h = fixture()
  h.options.create = root => ({ render: async () => {
    h.normal(root)
    root.children[1].rect = { width: 0, height: 0 }
  }, cancel: () => {} })
  const result = await renderSelectableTextLayer(h.options)
  assert.equal(result.dataset.textLayerEngine, 'fallback')
  assert.equal(copiedText(result), '正文 α 中文\nAnother line')
})

test('a collapsed text container on a visible page is recovered instead of mistaken for a hidden tab', async () => {
  const h = fixture()
  h.container.rect = { width: 0, height: 0 }
  const result = await renderSelectableTextLayer(h.options)
  assert.equal(result.dataset.textLayerEngine, 'fallback')
  assert.ok(hasSelectableText(result, content))
})

test('a hung text-layout promise is bounded and cancelled before recovery', async t => {
  t.mock.method(console, 'warn', () => {})
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const h = fixture()
  h.options.create = () => ({ render: () => new Promise(() => {}), cancel: () => {} })
  const pending = renderSelectableTextLayer(h.options)
  t.mock.timers.tick(TEXT_LAYER_TIMEOUT_MS)
  const result = await pending
  assert.equal(result.dataset.textLayerEngine, 'fallback')
  assert.equal(copiedText(result), '正文 α 中文\nAnother line')
})

test('a superseded render never installs a fallback in the new page', async () => {
  const h = fixture()
  h.options.cancelled = () => true
  h.options.create = () => ({ render: async () => { throw new Error('Cancelled') }, cancel: () => {} })
  const result = await renderSelectableTextLayer(h.options)
  assert.equal(result, h.container)
  assert.equal(h.page.children[0], h.container)
  assert.equal(result.children.length, 0)
})

test('hidden pages and image-only pages do not invent text or require OCR', async () => {
  const h = fixture(); h.page.rootConnected = false
  h.options.create = root => ({ render: async () => h.normal(root, { zero: true }), cancel: () => {} })
  assert.equal(await renderSelectableTextLayer(h.options), h.container)
  const image = fixture(); image.options.content = { items: [], styles: {} }
  image.options.create = () => ({ render: async () => {}, cancel: () => {} })
  assert.equal(await renderSelectableTextLayer(image.options), image.container)
  assert.equal(image.container.children.length, 0)
})

test('fallback geometry follows PDF coordinates, preserves line ends and scales with zoom', () => {
  const h = fixture()
  buildTextLayerFallback(h.container, content, viewport)
  const [span] = h.container.querySelectorAll()
  assert.equal(parseFloat(span.style.left), 100 * 50 / 600)
  assert.equal(parseFloat(span.style.top), 100 * 91 / 800)
  assert.equal(h.container.children[1].tag, 'br')
  assert.ok(span.style.fontSize.includes('--total-scale-factor'))
  assert.ok(h.container.style.width.includes('--total-scale-factor'))
  const transform = /scale\(([^,]+), ([^)]+)\)/.exec(span.style.transform)
  assert.equal(Number(transform[1]) * span.getBoundingClientRect().width, 80)
  assert.equal(Number(transform[2]) * span.getBoundingClientRect().height, 12)
})

test('page rotation and vertical text use transformed PDF baselines', () => {
  const h = fixture()
  buildTextLayerFallback(h.container, content, {
    width: 800, height: 600, scale: 1, transform: [0, 1, 1, 0, 0, 0],
  })
  const [span] = h.container.querySelectorAll()
  assert.equal(parseFloat(span.style.left), 100 * 709 / 800)
  assert.equal(parseFloat(span.style.top), 100 * 50 / 600)
  assert.ok(span.style.transform.startsWith('rotate(90deg)'))
  const vertical = { ...content, styles: { body: { fontFamily: 'serif', vertical: true, ascent: 0.75 } } }
  buildTextLayerFallback(h.container, vertical, viewport)
  assert.ok(h.container.querySelectorAll()[0].style.transform.startsWith('rotate(90deg)'))
})

test('PDF UserUnit scales recovered text width as well as font height and positions', () => {
  const h = fixture()
  buildTextLayerFallback(h.container, content, {
    width: 1200, height: 1600, scale: 1, transform: [2, 0, 0, -2, 0, 1600],
  })
  const [span] = h.container.querySelectorAll()
  const transform = /scale\(([^,]+), ([^)]+)\)/.exec(span.style.transform)
  assert.equal(Number(transform[1]) * span.getBoundingClientRect().width, 160)
  assert.equal(Number(transform[2]) * span.getBoundingClientRect().height, 24)
})
