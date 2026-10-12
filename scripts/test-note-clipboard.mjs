// Run: node --test scripts/test-note-clipboard.mjs. No system clipboard is changed.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const { installNoteClipboard } = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/noteClipboard.ts', import.meta.url), 'utf8'),
)).toString('base64'))

// Use the installed editor's actual copy handler, including its propagation
// stop and empty HTML write, with only selection/DOM/conversion mocked.
const irSource = await readFile(new URL('../node_modules/vditor/src/ts/ir/index.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('ir.ts', irSource, ts.ScriptTarget.Latest, true)
const copyMethod = ast.statements.find(ts.isClassDeclaration).members.find(n => n.name?.getText(ast) === 'copy')
const createCopy = new Function('getSelection', 'document', transpile(
  copyMethod.getText(ast).replace(/^private copy/, 'function copy') + '\nreturn copy;',
))

class ClipboardData {
  entries = new Map()
  get types() { return [...this.entries.keys()] }
  setData(type, value) { this.entries.set(type, value) }
  getData(type) { return this.entries.get(type) ?? '' }
  clearData(type) { this.entries.delete(type) }
}
const pasteIntoRichText = data => data.types.includes('text/html')
  ? data.getData('text/html') : data.getData('text/plain')

function fixture(markdown = '中文 **重点** 与 $x_i$。\n\n第二段。') {
  const editor = new EventTarget()
  let deletes = 0
  const range = { toString: () => markdown, cloneContents: () => ({}) }
  const copy = createCopy(() => ({ getRangeAt: () => range }), {
    createElement: () => ({ innerHTML: '<p>Selected contents</p>', appendChild() {} }),
  })
  const vditor = { lute: { VditorIRDOM2Md: () => markdown } }
  editor.addEventListener('copy', event => copy(event, vditor))
  editor.addEventListener('cut', event => { copy(event, vditor); deletes++ })
  const host = { querySelectorAll: () => [editor] }
  const dispatch = (type = 'copy') => {
    const event = new Event(type, { cancelable: true, bubbles: true })
    event.clipboardData = new ClipboardData()
    event.clipboardData.setData('application/x-fixture', 'retained')
    editor.dispatchEvent(event)
    return event
  }
  return { editor, host, dispatch, get deletes() { return deletes } }
}

test('note copy pastes its selected Markdown/TeX into a rich-text destination instead of empty HTML', () => {
  const h = fixture()
  assert.equal(pasteIntoRichText(h.dispatch().clipboardData), '') // reproduced bug
  const off = installNoteClipboard(h.host)
  const event = h.dispatch()
  assert.equal(event.defaultPrevented, true)
  assert.equal(pasteIntoRichText(event.clipboardData), '中文 **重点** 与 $x_i$。\n\n第二段。')
  assert.deepEqual(event.clipboardData.types, ['application/x-fixture', 'text/plain'])
  assert.equal(event.clipboardData.getData('application/x-fixture'), 'retained')
  off()
})

test('cut keeps the original editor deletion and fixes its clipboard without a second delete', () => {
  const h = fixture('just the selected fragment')
  installNoteClipboard(h.host)
  assert.equal(pasteIntoRichText(h.dispatch('cut').clipboardData), 'just the selected fragment')
  assert.equal(h.deletes, 1)
})

test('empty selections retain the default editor behavior', () => {
  const h = fixture('')
  installNoteClipboard(h.host)
  const event = h.dispatch()
  assert.equal(event.defaultPrevented, false)
  assert.deepEqual(event.clipboardData.types, ['application/x-fixture'])
})

test('valid HTML and unhandled browser copy are left intact', () => {
  for (const { html, handled } of [{ html: '<b>Text</b>', handled: true }, { html: '', handled: false }]) {
    const editor = new EventTarget()
    editor.addEventListener('copy', event => {
      event.clipboardData.setData('text/plain', 'Text')
      event.clipboardData.setData('text/html', html)
      if (handled) event.preventDefault()
    })
    installNoteClipboard({ querySelectorAll: () => [editor] })
    const event = new Event('copy', { cancelable: true })
    event.clipboardData = new ClipboardData()
    editor.dispatchEvent(event)
    assert.equal(event.clipboardData.getData('text/html'), html)
    assert.ok(event.clipboardData.types.includes('text/html'))
  }
})

test('binding is local to the editor and removal leaves no lingering repair', () => {
  const a = fixture('A'), b = fixture('B')
  const off = installNoteClipboard(a.host)
  assert.equal(pasteIntoRichText(a.dispatch().clipboardData), 'A')
  assert.equal(pasteIntoRichText(b.dispatch().clipboardData), '')
  off()
  assert.equal(pasteIntoRichText(a.dispatch().clipboardData), '')
})

test('missing clipboard data never prevents the default copy', () => {
  const editor = new EventTarget()
  installNoteClipboard({ querySelectorAll: () => [editor] })
  const event = new Event('copy', { cancelable: true })
  editor.dispatchEvent(event)
  assert.equal(event.defaultPrevented, false)
})
