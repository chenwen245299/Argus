// Run: node --test scripts/test-library-search.mjs
// Deterministic input/race regressions with the real controller and toolbar bindings.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { ref, watch, reactive, effectScope } from 'vue'
import { createPinia } from 'pinia'
import ts from 'typescript'

const transpile = code => ts.transpileModule(code, { compilerOptions: {
  target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS,
} }).outputText
const load = async path => {
  const api = {}
  new Function('exports', 'require', transpile(await readFile(new URL(path, import.meta.url), 'utf8')))(api, createRequire(import.meta.url))
  return api
}
const { createLibrarySearch } = await load('../src/utils/librarySearch.ts')
const { useSelectionStore } = await load('../src/stores/selection.ts')
const toolbar = await readFile(new URL('../src/components/Toolbar.vue', import.meta.url), 'utf8')
const bindings = toolbar.slice(toolbar.indexOf("const searchQuery = ref('')"), toolbar.indexOf('function shortPath'))
const handlers = toolbar.slice(toolbar.indexOf('function onSearchInput('), toolbar.indexOf('// ── Batch AI analysis'))
const mountBindings = new Function('ref', 'watch', 'library', 'selection', 'createLibrarySearch', 'invoke',
  transpile(`${bindings}\n${handlers}\nreturn { searchQuery, librarySearch, onSearchInput, onSearchEnter, clearSearch }`))
const flush = async () => { for (let n = 0; n < 5; n++) await Promise.resolve() }

function fixture(t) {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const requests = [], results = [], errors = []
  let root = '/library-a', busy = false, cleared = 0
  const controller = createLibrarySearch({
    root: () => root,
    search: (query, path) => new Promise((resolve, reject) => requests.push({ query, path, resolve, reject })),
    results: (query, hits) => results.push({ query, hits }),
    busy: value => busy = value,
    clear: () => cleared++,
    error: (query, error) => errors.push({ query, error }),
  })
  return { controller, requests, results, errors, tick: ms => t.mock.timers.tick(ms),
    get busy() { return busy }, get cleared() { return cleared }, root: value => root = value }
}

test('typing is debounced; Enter runs immediately and cancels the pending timer', async t => {
  const f = fixture(t)
  f.controller.schedule('energy')
  f.tick(100)
  f.controller.schedule('energy based')
  f.tick(179)
  assert.equal(f.requests.length, 0)
  f.controller.schedule(' energy based ', true)
  assert.equal(f.requests.length, 1)
  assert.equal(f.requests[0].query, 'energy based')
  f.requests[0].resolve(['paper'])
  await flush()
  f.tick(1000)
  assert.deepEqual(f.results, [{ query: 'energy based', hits: ['paper'] }])
  assert.equal(f.requests.length, 1)
  assert.equal(f.busy, false)
})

test('one request runs at a time; intermediate queued queries are discarded', async t => {
  const f = fixture(t)
  f.controller.schedule('first', true)
  f.controller.schedule('second', true)
  f.controller.schedule('latest', true)
  assert.equal(f.requests.length, 1)
  f.requests[0].resolve(['stale'])
  await flush()
  assert.equal(f.results.length, 0)
  assert.equal(f.busy, true)
  assert.equal(f.requests[1].query, 'latest')
  f.requests[1].resolve(['current'])
  await flush()
  assert.deepEqual(f.results, [{ query: 'latest', hits: ['current'] }])
  assert.equal(f.busy, false)
})

test('a response is stale as soon as input changes, even during debounce', async t => {
  const f = fixture(t)
  f.controller.schedule('old', true)
  f.controller.schedule('new')
  f.requests[0].resolve(['old paper'])
  await flush()
  assert.equal(f.results.length, 0)
  assert.equal(f.busy, true)
  f.tick(180)
  assert.equal(f.requests[1].query, 'new')
})

test('clear cancels scheduled work and ignores an already running response', async t => {
  const f = fixture(t)
  f.controller.schedule('pending')
  f.controller.clear()
  f.tick(1000)
  assert.equal(f.requests.length, 0)
  f.controller.schedule('running', true)
  f.controller.clear()
  f.requests[0].resolve(['old'])
  await flush()
  assert.equal(f.results.length, 0)
  assert.equal(f.busy, false)
  assert.equal(f.cleared, 2)
})

test('stale failures are ignored; current failures can be retried', async t => {
  const f = fixture(t)
  f.controller.schedule('old', true)
  f.controller.schedule('new', true)
  f.requests[0].reject('old failure')
  await flush()
  assert.equal(f.errors.length, 0)
  f.requests[1].reject('current failure')
  await flush()
  assert.deepEqual(f.errors, [{ query: 'new', error: 'current failure' }])
  assert.equal(f.busy, false)
  f.controller.schedule('new', true)
  f.requests[2].resolve(['recovered'])
  await flush()
  assert.equal(f.results[0].hits[0], 'recovered')
})

test('a result never crosses library roots', async t => {
  const f = fixture(t)
  f.controller.schedule('query', true)
  f.root('/library-b')
  f.requests[0].resolve(['library a paper'])
  await flush()
  assert.equal(f.results.length, 0)
})

function toolbarFixture(t) {
  const library = reactive({ currentPath: '/library-a' })
  const selection = useSelectionStore(createPinia()), requests = []
  const scope = effectScope()
  const ui = scope.run(() => mountBindings(ref, watch, library, selection, createLibrarySearch,
    (command, args) => new Promise((resolve, reject) => requests.push({ command, args, resolve, reject }))))
  t.after(() => { ui.librarySearch.cancel(); scope.stop() })
  return { ui, library, selection, requests, tick: ms => t.mock.timers.tick(ms) }
}

test('real toolbar ignores composing input and Enter; sends final text with its library', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const f = toolbarFixture(t)
  f.ui.searchQuery.value = '能量'
  f.ui.onSearchInput({ isComposing: true })
  f.ui.onSearchEnter({ isComposing: true })
  f.tick(1000)
  assert.equal(f.requests.length, 0)
  f.ui.onSearchInput({ isComposing: false })
  f.tick(180)
  assert.deepEqual(f.requests[0].args, { query: '能量', expectedRoot: '/library-a' })
  f.requests[0].resolve([{ slug: 'paper' }])
  await flush()
  assert.equal(f.selection.activeNav, 'search')
  assert.equal(f.selection.searchBusy, false)
  assert.equal(f.selection.searchResults[0].slug, 'paper')
})

test('real toolbar navigation, clearing and library switches invalidate old results', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  for (const action of ['navigate', 'clear', 'switch', 'dispose']) {
    const f = toolbarFixture(t)
    f.ui.searchQuery.value = 'energy'
    f.ui.onSearchEnter({ isComposing: false })
    if (action === 'navigate') f.selection.selectNav('recent')
    if (action === 'clear') f.ui.clearSearch()
    if (action === 'switch') f.library.currentPath = '/library-b'
    if (action === 'dispose') f.ui.librarySearch.cancel()
    f.requests[0].resolve([{ slug: 'stale' }])
    await flush()
    assert.equal(f.selection.searchResults.length, 0, action)
    assert.equal(f.selection.searchBusy, false, action)
    if (action === 'navigate') assert.equal(f.selection.activeNav, 'recent')
    if (action === 'switch') assert.equal(f.ui.searchQuery.value, '')
  }
})

test('leaving an existing search cancels a new result before it can reopen the view', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const f = toolbarFixture(t)
  f.selection.setSearchResults('previous', [{ slug: 'previous' }])
  f.ui.searchQuery.value = 'new query'
  f.ui.onSearchEnter({ isComposing: false })
  f.selection.selectNav('all')
  f.requests[0].resolve([{ slug: 'stale' }])
  await flush()
  assert.equal(f.selection.activeNav, 'all')
  assert.deepEqual(f.selection.searchResults, [{ slug: 'previous' }])
  assert.equal(f.selection.searchBusy, false)
})
