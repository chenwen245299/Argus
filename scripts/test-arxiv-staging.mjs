// Run: node --test scripts/test-arxiv-staging.mjs. No real library or network I/O.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { ref, computed, reactive } from 'vue'
import { createPinia, defineStore } from 'pinia'
import ts from 'typescript'

const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
async function astFor(path) {
  let source = await readFile(new URL(path, import.meta.url), 'utf8')
  if (path.endsWith('.vue')) source = source.slice(source.indexOf('>') + 1, source.indexOf('</script>'))
  return ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true)
}
function evaluate(source, env) {
  return new Function(...Object.keys(env), transpile(source))(...Object.values(env))
}
function functionText(ast, name) {
  let found
  const visit = node => {
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) found = node
    ts.forEachChild(node, visit)
  }
  visit(ast)
  assert.ok(found, `Function ${name} exists`)
  return found.getText(ast)
}
function variableText(ast, name) {
  const node = ast.statements.find(n => ts.isVariableStatement(n) && n.declarationList.declarations
    .some(d => ts.isIdentifier(d.name) && d.name.text === name))
  assert.ok(node, `Variable ${name} exists`)
  return node.getText(ast)
}
function storeFactory(ast, name, env) {
  const source = ast.statements.filter(n => !ts.isImportDeclaration(n))
    .map(n => n.getText(ast)).join('\n').replace(/^export /gm, '')
  return evaluate(`${source}\nreturn ${name};`, { defineStore, ref, computed, ...env })
}
const collectionsAst = await astFor('../src/stores/collections.ts')
const selectionAst = await astFor('../src/stores/selection.ts')
const arxivAst = await astFor('../src/stores/arxiv.ts')
const viewAst = await astFor('../src/views/ArxivView.vue')
const listAst = await astFor('../src/components/PaperList.vue')
const tabAst = await astFor('../src/components/TabBar.vue')
const paper = (id, source = 'arxiv') => ({ id, slug: id, title: id, import_source: source, tags: [] })
const initialCollections = () => ({
  collections: [{ id: 'top', name: 'Topic' }, { id: 'child', name: 'Category', parent_id: 'top' }],
  assignments: [],
})

async function harness(papers = [paper('a'), paper('b')]) {
  const library = reactive({ papers })
  let disk = initialCollections()
  const calls = []
  const invoke = async (name, args) => {
    calls.push({ name, args })
    if (name === 'get_collections') return structuredClone(disk)
    assert.equal(name, 'move_paper_to_collection')
    assert.equal(args.collectionId, 'child')
    disk.assignments = disk.assignments.filter(a => a.paper_id !== args.paperId)
    disk.assignments.push({ paper_id: args.paperId, collection_id: args.collectionId })
  }
  const useCollections = storeFactory(collectionsAst, 'useCollectionsStore', { invoke, useLibraryStore: () => library })
  const pinia = createPinia()
  const collectionsStore = useCollections(pinia)
  const selection = storeFactory(selectionAst, 'useSelectionStore', {})(pinia)
  await collectionsStore.load()
  selection.selectNav('arxiv')
  const filtered = evaluate(`${variableText(listAst, 'filtered')}\nreturn filtered;`, {
    computed, selection, library, collectionsStore, collectionPapers: ref([]),
  })
  return { library, collectionsStore, selection, filtered, calls, useCollections }
}

test('staging contains only unassigned recommendation imports, by paper identity', async () => {
  const h = await harness([paper('a'), paper('b'), paper('bio', 'biorxiv'), paper('local', 'file'), paper('url', 'url')])
  h.collectionsStore.file.assignments = [
    { paper_id: 'b', collection_id: 'child' },
    { paper_id: 'bio', collection_id: 'missing-collection' },
  ]
  assert.deepEqual(h.filtered.value.map(p => p.id), ['a', 'bio'])
  h.library.papers[0].reading_status = 'read'
  h.library.papers[0].slug = 'renamed-a'
  assert.deepEqual(h.filtered.value.map(p => p.id), ['a', 'bio'])
  h.selection.tagFilter = 'focus'
  h.library.papers[0].tags = ['focus']
  assert.deepEqual(h.filtered.value.map(p => p.id), ['a'])
  h.library.papers = h.library.papers.filter(p => p.id !== 'a')
  assert.equal(h.filtered.value.length, 0)
})

test('the arXiv navigation clears previous category, tags and selection, and labels the home tab', async () => {
  const h = await harness()
  h.selection.selectNav('collection:child')
  h.selection.tagFilter = 'old'
  h.selection.selectPaper('a')
  h.selection.selectNav('arxiv')
  assert.equal(h.selection.activeCollectionId, null)
  assert.equal(h.selection.highlightedCollectionId, null)
  assert.equal(h.selection.tagFilter, null)
  assert.deepEqual(h.selection.selectedSlugs, [])
  const title = evaluate(`${variableText(tabAst, 'homeTitle')}\nreturn homeTitle;`, {
    computed, selection: h.selection, collections: h.collectionsStore, t: key => key,
  })
  assert.equal(title.value, 'sidebar.arxivPapers')
})

function dragRunner(h, collectionId) {
  const listeners = new Map()
  const window = {
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: name => listeners.delete(name),
  }
  const document = {
    body: { style: {} }, dispatchEvent: () => {},
    elementsFromPoint: () => [{ dataset: { collectionId } }],
  }
  const onRowMouseDown = evaluate(`${functionText(listAst, 'onRowMouseDown')}\nreturn onRowMouseDown;`, {
    window, document, collectionsStore: h.collectionsStore, selection: h.selection,
    sorted: h.filtered, dragGhostItem: ref(null), dragGhostPos: ref({}),
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail } },
  })
  return async item => {
    onRowMouseDown({ button: 0, clientX: 0, clientY: 0 }, item)
    listeners.get('mousemove')({ clientX: 20, clientY: 20 })
    await listeners.get('mouseup')({ clientX: 20, clientY: 20 })
    assert.equal(listeners.size, 0)
  }
}

for (const multi of [false, true]) {
  test(`${multi ? 'multi-selection' : 'single-paper'} drag classifies papers, updates staging immediately, and survives reload`, async () => {
    const h = await harness()
    if (multi) h.selection.selectRange(['a', 'b'], 'a')
    await dragRunner(h, 'child')(h.filtered.value[0])
    assert.deepEqual(h.filtered.value.map(p => p.id), multi ? [] : ['b'])
    assert.equal(h.collectionsStore.collectionPaperCount('child'), multi ? 2 : 1)
    assert.equal(h.library.papers.length, 2)
    const reopened = h.useCollections(createPinia())
    await reopened.load()
    assert.deepEqual(reopened.arxivPapers.map(p => p.id), multi ? [] : ['b'])
  })
}

test('dragging onto a top-level category leaves staging intact', async () => {
  const h = await harness()
  await dragRunner(h, 'top')(h.filtered.value[0])
  assert.equal(h.filtered.value.length, 2)
  assert.equal(h.calls.filter(c => c.name === 'move_paper_to_collection').length, 0)
})

function importRunner(outcomes, confirm = () => true) {
  const papers = ref([{ arxiv_id: 'a' }])
  const calls = []
  const addToLibrary = evaluate(`${functionText(arxivAst, 'addToLibrary')}\nreturn addToLibrary;`, {
    papers, confirmDuplicateImport: async () => await confirm(),
    invoke: async (name, args) => { calls.push({ name, args }); return outcomes.shift() },
  })
  return { papers, calls, addToLibrary }
}

test('recommendation import and confirmed duplicate retries never send a collection', async () => {
  for (const duplicate of [false, true]) {
    const outcomes = duplicate ? [{ status: 'duplicate', title: 'Existing' }] : []
    outcomes.push({ status: 'imported', slug: 'new-copy' })
    const h = importRunner(outcomes)
    assert.equal(await h.addToLibrary('a'), 'new-copy')
    assert.equal(h.papers.value.length, 0)
    assert.deepEqual(h.calls.map(c => c.args), duplicate ? [{ arxivId: 'a' }, { arxivId: 'a', force: true }] : [{ arxivId: 'a' }])
  }
})

test('canceling a duplicate keeps the recommendation in the inbox', async () => {
  const h = importRunner([{ status: 'duplicate' }], () => false)
  assert.equal(await h.addToLibrary('a'), null)
  assert.equal(h.papers.value.length, 1)
  assert.equal(h.calls.length, 1)
})

test('recommendations wait for an asynchronous duplicate choice and stay in place if the native dialog fails', async () => {
  let resolve
  const choice = new Promise(done => { resolve = done })
  const h = importRunner([{ status: 'duplicate' }], () => choice)
  const importing = h.addToLibrary('a')
  await new Promise(done => setImmediate(done))
  assert.equal(h.calls.length, 1)
  assert.equal(h.papers.value.length, 1)
  resolve(false)
  assert.equal(await importing, null)
  const failed = importRunner([{ status: 'duplicate' }], () => { throw new Error('dialog failed') })
  await assert.rejects(failed.addToLibrary('a'), /dialog failed/)
  assert.equal(failed.calls.length, 1)
  assert.equal(failed.papers.value.length, 1)
})

test('the direct add action refreshes the main window; canceled or failed imports restore selection', async () => {
  for (const outcome of ['success', 'canceled', 'failed']) {
    const selectedId = ref('a'), addingId = ref(null), events = [], messages = []
    const add = evaluate(`${functionText(viewAst, 'addToLibrary')}\nreturn addToLibrary;`, {
      selectedId, addingId, filteredPapers: ref([{ arxiv_id: 'a' }, { arxiv_id: 'b' }]),
      setAddMessage: (...args) => messages.push(args), t: key => key,
      emitTo: async (...args) => events.push(args),
      store: { addToLibrary: async (...args) => {
        assert.deepEqual(args, ['a'])
        if (outcome === 'failed') throw new Error('download failed')
        return outcome === 'success' ? 'new-paper' : null
      } },
    })
    await add({ arxiv_id: 'a', title: 'Paper A' })
    assert.equal(addingId.value, null)
    assert.equal(selectedId.value, outcome === 'success' ? 'b' : 'a')
    assert.equal(events.length, outcome === 'success' ? 1 : 0)
    if (outcome === 'success') {
      assert.deepEqual(events[0], ['main', 'library-paper-added', { slug: 'new-paper', title: 'Paper A' }])
      assert.deepEqual(messages.at(-1), ['arxiv.addedToStaging', 3000])
    }
  }
})
