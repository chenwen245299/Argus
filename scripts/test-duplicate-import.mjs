// Run: node --test scripts/test-duplicate-import.mjs
// Exercise the real import store with isolated command/dialog mocks.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { webcrypto } from 'node:crypto'
import { ref, computed } from 'vue'
import ts from 'typescript'
import { confirm as nativeConfirm } from '@tauri-apps/plugin-dialog'

const transpile = code => ts.transpileModule(code, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const source = await readFile(new URL('../src/stores/import.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('import.ts', source, ts.ScriptTarget.Latest, true)
const store = ast.statements.find(n => ts.isVariableStatement(n) &&
  n.declarationList.declarations.some(d => d.name.getText(ast) === 'useImportStore'))
  .declarationList.declarations[0].initializer.arguments[1].getText(ast)
const createStore = new Function('env', transpile(`
  const { ref, computed, invoke, listen, window, CustomEvent, crypto, i18n,
    useLibraryStore, useReaderStore, useSelectionStore, useCollectionsStore,
    useRanksStore, useSettingsStore, confirmDuplicateImport } = env;
  return (${store})();
`))

function fixture(accept, duplicate = true, deleteFails = false) {
  const calls = [], questions = [], moves = [], renames = []
  const library = { papers: [], refresh: async () => {} }
  const selection = { selectedSlug: '', selectPaper(slug) { this.selectedSlug = slug } }
  const collections = { canReceivePapers: () => true, load: async () => {},
    movePaper: async (id, collection) => { moves.push({ id, collection }) } }
  const env = {
    ref, computed, crypto: webcrypto,
    window: { confirm() { throw new Error('Deprecated window.confirm must not be used') }, dispatchEvent() {} },
    confirmDuplicateImport: async title => { questions.push(`Import ${title} as an independent copy?`); return typeof accept === 'function' ? await accept() : await accept },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail } },
    i18n: { global: { t: (_key, vars = {}) => `Import ${vars.title} as an independent copy?` } },
    listen: async () => () => {},
    useLibraryStore: () => library, useSelectionStore: () => selection,
    useCollectionsStore: () => collections,
    useReaderStore: () => ({ replacePaperSlug: (a, b) => renames.push([a, b]) }),
    useRanksStore: () => ({}), useSettingsStore: () => ({ loaded: true, easyscholarConfigured: false }),
    invoke: async (name, args) => {
      calls.push({ name, args })
      switch (name) {
        case 'delete_paper': if (deleteFails) throw new Error('delete failed'); return null
        case 'import_pdf': return 'new-temp'
        case 'import_ebook': return 'ebook-copy'
        case 'import_paper_url': return !args.force && duplicate
          ? { status: 'duplicate', existingSlug: 'original', title: 'Same Paper' }
          : { status: 'imported', slug: 'url-copy' }
        case 'get_paper_meta': return { id: `uuid-${args.slug}`, title: 'Same Paper' }
        case 'find_duplicate_paper': return duplicate ? { existingSlug: 'original', title: 'Same Paper' } : null
        case 'rename_paper_folder': return args.slug === 'new-temp' ? 'same-paper-2' : args.slug
        case 'extract_metadata_ai': return { id: `uuid-${args.slug}`, title: 'Same Paper' }
        default: return null
      }
    },
  }
  return { store: createStore(env), calls, questions, moves, renames, selection }
}
async function idle(store) {
  for (let i = 0; i < 100 && store.activeCount.value; i++) await new Promise(resolve => setImmediate(resolve))
  assert.equal(store.activeCount.value, 0, 'metadata worker finishes')
}

test('a local duplicate is kept only after approval and receives its own slug/UUID', async () => {
  const f = fixture(true)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.questions.length, 1)
  assert.match(f.questions[0], /Same Paper/)
  assert.equal(f.store.jobs.value[0].slug, 'same-paper-2')
  assert.equal(f.store.jobs.value[0].status, 'done')
  assert.deepEqual(f.moves, [{ id: 'uuid-new-temp', collection: 'leaf' }])
  assert.deepEqual(f.renames, [['new-temp', 'same-paper-2']])
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  assert.equal(f.selection.selectedSlug, 'same-paper-2')
})

test('declining a local duplicate deletes only the newly imported copy', async () => {
  const f = fixture(false)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.deepEqual(f.calls.filter(c => c.name === 'delete_paper'), [{ name: 'delete_paper', args: { slug: 'new-temp' } }])
  assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), false)
  assert.equal(f.selection.selectedSlug, 'original')
})

test('accepting a URL duplicate retries with force and is not asked again during metadata', async () => {
  const f = fixture(true)
  await f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  await idle(f.store)
  const imports = f.calls.filter(c => c.name === 'import_paper_url')
  assert.equal(imports.length, 2)
  assert.equal(imports[0].args.force, undefined)
  assert.equal(imports[1].args.force, true)
  assert.equal(f.questions.length, 1)
  assert.equal(f.calls.some(c => c.name === 'find_duplicate_paper' || c.name === 'delete_paper'), false)
  assert.equal(f.selection.selectedSlug, 'url-copy')
})

test('declining a URL duplicate never forces a download or deletes the existing paper', async () => {
  const f = fixture(false)
  await f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  assert.equal(f.store.jobs.value.length, 0)
})

test('ebooks use the same choice and keep separate copies', async () => {
  for (const accept of [true, false]) {
    const f = fixture(accept)
    await f.store.importFile('/tmp/book.epub', 'leaf')
    assert.equal(f.questions.length, 1)
    assert.equal(f.calls.some(c => c.name === 'delete_paper'), !accept)
    if (accept) assert.equal(f.selection.selectedSlug, 'ebook-copy')
  }
})

test('ordinary new imports proceed without a duplicate prompt', async () => {
  const f = fixture(false, false)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.questions.length, 0)
  assert.equal(f.store.jobs.value[0].status, 'done')
})

const deferred = () => {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}
async function until(predicate) {
  for (let i = 0; i < 100 && !predicate(); i++) await new Promise(resolve => setImmediate(resolve))
  assert.ok(predicate())
}

test('a local import pauses until the actual asynchronous duplicate choice arrives', async () => {
  for (const accept of [false, true]) {
    const choice = deferred(), f = fixture(choice.promise)
    await f.store.importFile('/tmp/paper.pdf', 'leaf')
    await until(() => f.questions.length === 1)
    assert.ok(f.store.activeCount.value > 0)
    assert.equal(f.calls.some(c => ['rename_paper_folder', 'fetch_citation_count', 'delete_paper'].includes(c.name)), false)
    choice.resolve(accept)
    await idle(f.store)
    assert.equal(f.calls.some(c => c.name === 'delete_paper'), !accept)
    assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), accept)
  }
})

test('URL duplicates cannot force a copy before the asynchronous user choice', async () => {
  const choice = deferred(), f = fixture(choice.promise)
  const importing = f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  await until(() => f.questions.length === 1)
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  choice.resolve(false)
  await importing
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(f.store.jobs.value.length, 0)
})

test('a dialog failure never silently approves a known local or URL duplicate', async () => {
  const fail = () => { throw new Error('dialog failed') }
  const local = fixture(fail)
  await local.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(local.store)
  assert.equal(local.store.jobs.value[0].status, 'error')
  assert.equal(local.calls.some(c => ['rename_paper_folder', 'fetch_citation_count', 'delete_paper'].includes(c.name)), false)
  const url = fixture(fail)
  await url.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  assert.equal(url.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(url.store.jobs.value[0].status, 'error')
  const ebook = fixture(fail)
  await ebook.store.importFile('/tmp/book.epub', 'leaf')
  assert.equal(ebook.store.jobs.value[0].status, 'error')
  assert.equal(ebook.store.activeCount.value, 0)
  assert.equal(ebook.calls.some(c => c.name === 'delete_paper'), false)
})

test('a failed cleanup is reported as an error and never claims the import was skipped or deletes the original', async () => {
  const f = fixture(false, true, true)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.store.jobs.value[0].status, 'error')
  assert.equal(f.store.lastDuplicate.value, null)
  assert.deepEqual(f.calls.filter(c => c.name === 'delete_paper'), [{ name: 'delete_paper', args: { slug: 'new-temp' } }])
  assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), false)
})

test('the real dialog helper uses the supported native command and localized keep/skip buttons', async () => {
  const helper = await readFile(new URL('../src/utils/duplicateImport.ts', import.meta.url), 'utf8')
  const helperAst = ts.createSourceFile('duplicateImport.ts', helper, ts.ScriptTarget.Latest, true)
  const fn = helperAst.statements.find(n => ts.isFunctionDeclaration(n)).getText(helperAst).replace(/^export /, '')
  const translations = { 'import.duplicateConfirm': 'Same-title paper: {title}', 'import.duplicateTitle': '论文可能重复',
    'import.keepDuplicate': '保留副本', 'import.skipDuplicate': '跳过导入' }
  const confirmDuplicate = new Function('confirm', 'i18n', transpile(`${fn}\nreturn confirmDuplicateImport;`))(
    nativeConfirm, { global: { t: (key, args = {}) => translations[key].replace('{title}', args.title ?? '') } })
  const previousWindow = globalThis.window
  try {
    for (const accept of [true, false]) {
      const choice = deferred(), calls = []
      globalThis.window = { confirm() { throw new Error('Broken shim') }, __TAURI_INTERNALS__: {
        invoke(command, args) { calls.push({ command, args }); return choice.promise },
      } }
      const result = confirmDuplicate('Same Paper')
      assert.equal(calls[0].command, 'plugin:dialog|message')
      assert.equal(calls[0].args.title, '论文可能重复')
      assert.ok(JSON.stringify(calls[0].args.buttons).includes('保留副本'))
      assert.ok(JSON.stringify(calls[0].args.buttons).includes('跳过导入'))
      choice.resolve(accept ? '保留副本' : '跳过导入')
      assert.equal(await result, accept)
    }
  } finally { globalThis.window = previousWindow }
})
