// Run: node --test scripts/test-duplicate-import.mjs
// Exercise the real import store with isolated command/dialog mocks.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { webcrypto } from 'node:crypto'
import { ref, computed } from 'vue'
import ts from 'typescript'

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
    useRanksStore, useSettingsStore } = env;
  return (${store})();
`))

function fixture(accept, duplicate = true) {
  const calls = [], questions = [], moves = [], renames = []
  const library = { papers: [], refresh: async () => {} }
  const selection = { selectedSlug: '', selectPaper(slug) { this.selectedSlug = slug } }
  const collections = { canReceivePapers: () => true, load: async () => {},
    movePaper: async (id, collection) => { moves.push({ id, collection }) } }
  const env = {
    ref, computed, crypto: webcrypto,
    window: { confirm(message) { questions.push(message); return accept }, dispatchEvent() {} },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail } },
    i18n: { global: { t: (_key, vars) => `Import ${vars.title} as an independent copy?` } },
    listen: async () => () => {},
    useLibraryStore: () => library, useSelectionStore: () => selection,
    useCollectionsStore: () => collections,
    useReaderStore: () => ({ replacePaperSlug: (a, b) => renames.push([a, b]) }),
    useRanksStore: () => ({}), useSettingsStore: () => ({ loaded: true, easyscholarConfigured: false }),
    invoke: async (name, args) => {
      calls.push({ name, args })
      switch (name) {
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
