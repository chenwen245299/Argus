// Run: node --test scripts/test-page-furniture.mjs. No PDF, browser or library is required.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { webcrypto } from 'node:crypto'
import ts from 'typescript'

const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const furniture = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/pageFurniture.ts', import.meta.url), 'utf8'),
)).toString('base64'))

// Exercise the viewer's real copy/save handlers and planner, with the DOM bridge
// replaced by already-read text layers. The classification and drop policy are real.
const viewer = await readFile(new URL('../src/components/PdfViewer.vue', import.meta.url), 'utf8')
const setupStart = viewer.indexOf('>', viewer.indexOf('<script setup')) + 1
const ast = ts.createSourceFile('viewer.ts', viewer.slice(setupStart, viewer.indexOf('</script>', setupStart)), ts.ScriptTarget.Latest, true)
const functions = ['sameText', 'planSelectionFurniture', 'ownsTextSelection', 'onCopySelection', 'refreshSelectionFurniture', 'createHighlight']
  .map(name => {
    const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name)
    assert.ok(node, `Viewer handler ${name} exists`)
    return node.getText(ast)
  }).join('\n')
const makeHandlers = new Function('env', transpile(`
  const { window, Node, crypto, pageRefs, pageCount, docBodyFontSize,
    furnitureReading, renderedFurniturePage, selectedTextBySpan,
    keptSelectionTextAcrossPages, planFurnitureDropReport,
    collectSelectionRectsByPage, selectionPopup, showSkippedSpans,
    reader, activeColor, highlightStyle } = env;
  let popupSource = env.popupSource;
  ${functions}
  return { onCopySelection, createHighlight };
`))

const span = (text, x, y, w, fontSize = 10) => ({ text, x, y, w, h: fontSize, fontSize })
function fixture() {
  const body = Array.from({ length: 20 }, (_, i) => span(
    `Body line ${i}: The main paragraph describes a model and explains how its predictions are evaluated.`, 108, 150 + i * 20, 396,
  ))
  const first = { width: 612, height: 792, spans: [...body,
    span('2', 120, 650, 3, 6),
    span('Recent work adds a separate explanatory note about this procedure.', 124, 652, 380, 9),
    span('The continuation belongs to the footnote rather than to the paragraph.', 108, 662, 396, 9),
    span('This is the end of the note.', 108, 672, 200, 9),
    span('3', 303.5, 742, 5),
  ] }
  const second = { width: 612, height: 792, spans: [
    span('Figure 2: An example diagram. Each box represents a stage of this procedure.', 108, 230, 396),
    span('The arrows show how the stages connect and how information flows between them.', 108, 241, 396),
    span('This line ends the caption.', 108, 252, 200),
    span('To achieve these goals, we propose a model that evaluates candidate predictions.', 108, 282, 396),
    span('The following sentence continues the main paragraph across the page boundary.', 108, 293, 396),
    span('4', 303.5, 742, 5),
  ] }
  const selected = [
    new Map(first.spans.slice(19).map((s, i) => [19 + i, s.text])),
    new Map(second.spans.slice(0, 5).map((s, i) => [i, s.text])),
  ]
  return { pages: [first, second], selected }
}

function harness({ singlePage = false, outside = false, mismatch = false, plainPopup = false, endInCaption = false } = {}) {
  const { pages, selected } = fixture()
  if (endInCaption) selected[1] = new Map([...selected[1]].filter(([i]) => i < 2))
  if (singlePage) { pages.pop(); selected.pop() }
  const layers = pages.map(() => ({}))
  const elements = pages.map(p => p.spans.map(() => ({})))
  const readings = pages.map((page, i) => ({ page, elements: elements[i], eolAfter: page.spans.map(() => true) }))
  const text = furniture.keptSelectionTextAcrossPages(readings.map((r, i) => ({ selected: selected[i], eolAfter: r.eolAfter })))
  const endpoint = i => ({ nodeType: 1, closest: () => outside ? {} : layers[i] })
  const range = { startContainer: endpoint(0), endContainer: endpoint(pages.length - 1) }
  const selection = { isCollapsed: false, rangeCount: 1, getRangeAt: () => range, toString: () => text + (mismatch ? 'unexpected text' : ''), removeAllRanges() {} }
  const collect = (_range, skip) => pages.map((_, pi) => ({ pageIndex: pi, rects: [...selected[pi].keys()]
    .filter(i => !skip?.has(elements[pi][i])).map(i => ({ x: i, y: 0, width: 1, height: 1 })) }))
  const popup = { value: plainPopup ? { text, pages: collect(range), x: 10, y: 10 } : null }
  const saved = [], shown = []
  const handlers = makeHandlers({
    window: { getSelection: () => selection }, Node: { ELEMENT_NODE: 1 }, crypto: webcrypto,
    pageRefs: { value: layers.map(layer => ({ contains: l => layer === l, querySelector: () => layer })) },
    pageCount: { value: pages.length }, docBodyFontSize: 10,
    furnitureReading: i => readings[i], renderedFurniturePage: () => null,
    selectedTextBySpan: (_range, els) => selected[elements.indexOf(els)],
    keptSelectionTextAcrossPages: furniture.keptSelectionTextAcrossPages,
    planFurnitureDropReport: furniture.planFurnitureDropReport,
    collectSelectionRectsByPage: collect, selectionPopup: popup,
    popupSource: plainPopup ? { range, skip: null } : null,
    showSkippedSpans: spans => shown.push(...spans),
    reader: { addHighlights: highlights => saved.push(...highlights) },
    activeColor: { value: '#ffaa00' }, highlightStyle: { value: 'highlight' },
  })
  const copy = () => {
    let copied = null, prevented = false
    handlers.onCopySelection({ clipboardData: { setData: (_type, value) => { copied = value } }, preventDefault: () => { prevented = true } })
    return { copied, prevented }
  }
  return { ...handlers, copy, text, saved, shown, popup }
}

function assertBodyOnly(text) {
  assert.ok(text.includes('Body line 19:'))
  assert.ok(text.includes('To achieve these goals'))
  assert.ok(!text.includes('explanatory note'))
  assert.ok(!text.includes('\n3\n'))
  assert.ok(!text.includes('Figure 2:'))
}

test('cross-page copy filters without a mouse-up popup', () => {
  const h = harness()
  const { copied, prevented } = h.copy()
  assert.ok(prevented)
  assertBodyOnly(copied)
})

test('saving rechecks a plain popup and removes furniture from both text and rects', () => {
  const h = harness({ plainPopup: true })
  h.createHighlight()
  assert.equal(h.saved.length, 2)
  assertBodyOnly(h.saved[0].text)
  assert.equal(h.saved[0].text, h.saved[1].text)
  assert.equal(h.saved[0].created_at, h.saved[1].created_at)
  assert.deepEqual(h.saved.map(p => p.rects.map(r => r.x)), [[19], [3, 4]])
  assert.ok(h.shown.length > 0)
})

test('copy preserves the caption the user deliberately ends in', () => {
  const { copied, prevented } = harness({ endInCaption: true }).copy()
  assert.ok(prevented)
  assert.ok(copied.includes('Figure 2:'))
  assert.ok(!copied.includes('explanatory note'))
  assert.ok(!copied.includes('\n3\n'))
})

for (const [label, options] of [
  ['single-page selection', { singlePage: true }],
  ['note or other viewer selection', { outside: true }],
  ['text layer that does not match the browser selection', { mismatch: true }],
]) {
  test(`copy leaves ${label} to the browser`, () => {
    assert.deepEqual(harness(options).copy(), { copied: null, prevented: false })
  })
}
