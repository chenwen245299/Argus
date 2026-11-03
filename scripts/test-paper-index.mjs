import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
import { reactive, ref, watch, nextTick, effectScope } from 'vue'

const api = {}
new Function('exports', ts.transpileModule(await readFile(new URL('../src/utils/paperIndex.ts', import.meta.url), 'utf8'), {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS },
}).outputText)(api)
const { reconcilePaperIndex, reuseItemArray } = api
const clone = value => JSON.parse(JSON.stringify(value))
const entry = (slug = 'paper') => ({ id: 'id-' + slug, slug, title: 'Original', authors: ['A'], tags: ['Method'],
  year: 2026, venue: 'Nature', reading_status: 'unread', added_at: '2026-01-01', meta_mtime: 7,
  related_ids: ['related'], has_bibtex: false,
  status: { metadata_fetched: true, text_extracted: true, ai_summary_done: false, vectorized: false, last_updated: '2026-01-01' },
})

test('equivalent scans retain the array, reactive rows and unchanged nested values', () => {
  const previous = reactive([entry(), entry('second')]), first = previous[0], tags = first.tags, status = first.status
  const incoming = clone(previous)
  incoming[0].status = Object.fromEntries(Object.entries(incoming[0].status).reverse())
  const next = reconcilePaperIndex(previous, incoming)
  assert.equal(next, previous); assert.equal(next[0], first); assert.equal(first.tags, tags); assert.equal(first.status, status)
})
test('real updates and removed optional fields propagate even when meta mtime did not change', () => {
  const previous = reactive([entry()]), first = previous[0], incoming = clone(previous)
  incoming[0].title = 'Renamed'; incoming[0].authors = ['B']; incoming[0].tags = []
  incoming[0].status.ai_summary_done = true; incoming[0].related_ids = []; incoming[0].cite_count = 0
  delete incoming[0].year; delete incoming[0].venue
  assert.equal(reconcilePaperIndex(previous, incoming), previous)
  assert.equal(previous[0], first); assert.equal(first.title, 'Renamed'); assert.equal(first.year, undefined); assert.equal(first.venue, undefined)
  assert.deepEqual(first.authors, ['B']); assert.deepEqual(first.tags, []); assert.equal(first.status.ai_summary_done, true)
  assert.deepEqual(first.related_ids, []); assert.equal(first.cite_count, 0)
})
test('locally mutated objects are compared with the actual scan, not a stale fingerprint', () => {
  const previous = reactive([entry()]), incoming = clone(previous)
  previous[0].tags.push('Local only'); previous[0].status.vectorized = true
  reconcilePaperIndex(previous, incoming)
  assert.deepEqual(previous[0].tags, ['Method']); assert.equal(previous[0].status.vectorized, false)
})
test('membership/order changes are preserved and UUID/slug replacements never reuse another entry', () => {
  const previous = reactive([entry('first'), entry('second')]), incoming = clone(previous).reverse()
  const reordered = reconcilePaperIndex(previous, incoming)
  assert.notEqual(reordered, previous); assert.equal(reordered[0], previous[1]); assert.equal(reordered[1], previous[0])
  const replacement = { ...entry('first'), id: 'new-copy-id' }
  assert.equal(reconcilePaperIndex(previous, [replacement])[0], replacement)
  assert.equal(reconcilePaperIndex(previous, [{ ...entry('first'), slug: 'renamed-slug' }])[0].slug, 'renamed-slug')
  assert.deepEqual(reconcilePaperIndex(previous, []), [])
})
test('derived visible lists reuse their input only for identical row order', () => {
  const rows = [entry('a'), entry('b')]
  assert.equal(reuseItemArray(rows, [...rows]), rows)
  assert.notEqual(reuseItemArray(rows, [...rows].reverse()), rows)
  assert.notEqual(reuseItemArray(rows, clone(rows)), rows)
})

for (const path of ['../src/components/CanvasPanel.vue', '../src/views/CanvasView.vue']) {
  test(`${path}: real paper edits refresh canvas metadata while unchanged scans leave nodes alone`, async () => {
    const source = await readFile(new URL(path, import.meta.url), 'utf8')
    const start = source.indexOf('watch(() => library.papers'), end = source.indexOf('</script>', start)
    const code = ts.transpileModule(source.slice(start, end), { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText
    const library = reactive({ papers: [entry()] }), nodes = ref([{ type: 'paper', data: { paperId: 'id-paper', title: 'Original', authors: ['A'], year: 2026 } }, { type: 'text', data: { text: 'Keep my annotation' } }])
    const scope = effectScope(), annotation = nodes.value[1]
    scope.run(() => new Function('watch', 'library', 'nodes', 'paperById', 'canvas', 'loadNoteTitles', 'noteTitlesMap', code)(
      watch, library, nodes, id => library.papers.find(p => p.id === id), ref(null), () => {}, ref(new Map())))
    try {
      const before = nodes.value
      library.papers = reconcilePaperIndex(library.papers, clone(library.papers)); await nextTick()
      assert.equal(nodes.value, before)
      const updated = clone(library.papers); updated[0].title = 'Changed title'; updated[0].authors = ['B']; updated[0].year = 2024
      library.papers = reconcilePaperIndex(library.papers, updated); await nextTick()
      assert.equal(nodes.value[0].data.title, 'Changed title'); assert.deepEqual(nodes.value[0].data.authors, ['B']); assert.equal(nodes.value[0].data.year, 2024)
      assert.equal(nodes.value[1], annotation)
    } finally { scope.stop() }
  })
}
