// Run: node --test scripts/test-paper-marking.mjs
// The real reader merge keeps independent copies and unsaved local edits apart.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { ref } from 'vue'
import ts from 'typescript'

const source = await readFile(new URL('../src/stores/reader.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('reader.ts', source, ts.ScriptTarget.Latest, true)
const callback = ast.statements.find(n => ts.isVariableStatement(n) &&
  n.declarationList.declarations.some(d => d.name.getText(ast) === 'useReaderStore'))
  .declarationList.declarations[0].initializer.arguments[1]
const functions = ['setHighlights', 'mergeAddedHighlights'].map(name =>
  callback.body.statements.find(n => ts.isFunctionDeclaration(n) && n.name.text === name).getText(ast)).join('\n')
const createMerge = new Function('env', ts.transpileModule(`
  const { tabs, highlightsBySlug, highlightTombstones } = env;
  ${functions}
  return mergeAddedHighlights;
`, { compilerOptions: { target: ts.ScriptTarget.ES2021 } }).outputText)
const mark = (id, note = null, time = '2026-01-01') => ({
  id, page: 1, text: 'Same passage', rects: [], color: '#FFEB3B', note, created_at: time,
})

test('agent highlights update the exact open copy, even while another copy is active', () => {
  const original = mark('user-original', 'Original note')
  const env = { tabs: ref([{ slug: 'same-paper' }, { slug: 'same-paper-2' }]),
    highlightsBySlug: ref({ 'same-paper': [original], 'same-paper-2': [] }), highlightTombstones: ref({}) }
  const merge = createMerge(env)
  merge('same-paper-2', [mark('agent-copy', 'Copy annotation')])
  assert.deepEqual(env.highlightsBySlug.value['same-paper'], [original])
  assert.equal(env.highlightsBySlug.value['same-paper-2'][0].id, 'agent-copy')
  merge('closed-paper', [mark('closed')])
  assert.equal(env.highlightsBySlug.value['closed-paper'], undefined, 'closed readers load disk when opened')
})

test('retry events retain newer local notes and pending local deletions', () => {
  const local = { ...mark('shared', 'Unsaved local annotation'), updated_at: '2026-01-03' }
  const env = { tabs: ref([{ slug: 'paper' }]), highlightsBySlug: ref({ paper: [local] }),
    highlightTombstones: ref({ paper: [{ id: 'deleted', deleted_at: '2026-01-02' }] }) }
  const merge = createMerge(env)
  merge('paper', [mark('shared', 'Older note'), mark('deleted'), mark('new')])
  assert.equal(env.highlightsBySlug.value.paper.length, 2)
  assert.equal(env.highlightsBySlug.value.paper.find(h => h.id === 'shared').note, 'Unsaved local annotation')
  assert.equal(env.highlightsBySlug.value.paper.some(h => h.id === 'deleted'), false)
})
