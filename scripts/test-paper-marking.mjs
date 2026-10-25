// Run: node --test scripts/test-paper-marking.mjs
// The real reader merge keeps independent copies and unsaved local edits apart,
// for new agent marks and for approved changes / retractions of existing ones.
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
const functions = ['setHighlights', 'mergeAgentHighlights'].map(name =>
  callback.body.statements.find(n => ts.isFunctionDeclaration(n) && n.name.text === name).getText(ast)).join('\n')
const createMerge = new Function('env', ts.transpileModule(`
  const { tabs, highlightsBySlug, highlightTombstones } = env;
  ${functions}
  return mergeAgentHighlights;
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

test('approved changes replace older local copies, while newer local edits and pending deletions survive', () => {
  const env = { tabs: ref([{ slug: 'paper' }]), highlightTombstones: ref({ paper: [{ id: 'deleted', deleted_at: '2026-01-02' }] }),
    highlightsBySlug: ref({ paper: [mark('recolored'), { ...mark('edited-here', 'Unsaved local note'), updated_at: '2026-01-05' },
      mark('other', 'Untouched')] }) }
  const merge = createMerge(env)
  merge('paper', [
    { ...mark('recolored', 'Revised note'), color: '#CE93D8', updated_at: '2026-01-03', agent: { revision: '2026-01-03' } },
    { ...mark('edited-here', 'Agent note'), updated_at: '2026-01-03' },
    { ...mark('deleted', 'Agent note'), updated_at: '2026-01-03' },
  ], [])
  const byId = Object.fromEntries(env.highlightsBySlug.value.paper.map(h => [h.id, h]))
  assert.deepEqual(Object.keys(byId).sort(), ['edited-here', 'other', 'recolored'])
  assert.equal(byId.recolored.color, '#CE93D8')
  assert.equal(byId.recolored.note, 'Revised note')
  assert.deepEqual(byId.recolored.agent, { revision: '2026-01-03' }, 'provenance travels with the record')
  assert.equal(byId['edited-here'].note, 'Unsaved local note')
  assert.equal(byId.other.note, 'Untouched')
})

test('retractions drop unedited marks and record tombstones, but never a mark edited here afterwards', () => {
  const env = { tabs: ref([{ slug: 'paper' }, { slug: 'copy' }]), highlightTombstones: ref({}),
    highlightsBySlug: ref({ paper: [mark('retracted'), { ...mark('edited-after', 'Mine now'), updated_at: '2026-01-04' }, mark('kept')],
      copy: [mark('retracted')] }) }
  const merge = createMerge(env)
  merge('paper', [], [{ id: 'retracted', deleted_at: '2026-01-03' }, { id: 'edited-after', deleted_at: '2026-01-03' }])
  assert.deepEqual(env.highlightsBySlug.value.paper.map(h => h.id).sort(), ['edited-after', 'kept'])
  assert.deepEqual(env.highlightTombstones.value.paper, [{ id: 'retracted', deleted_at: '2026-01-03' }])
  assert.deepEqual(env.highlightsBySlug.value.copy.map(h => h.id), ['retracted'], 'another copy of the paper is untouched')
  merge('closed-paper', [], [{ id: 'x', deleted_at: '2026-01-03' }])
  assert.equal(env.highlightTombstones.value['closed-paper'], undefined, 'closed readers load disk when opened')
  // A repeated event changes nothing and does not duplicate the tombstone.
  merge('paper', [], [{ id: 'retracted', deleted_at: '2026-01-03' }])
  assert.equal(env.highlightTombstones.value.paper.length, 1)
})

test('paper and library chat forward the approved color list against the exact queued request, while rejections omit edits', async () => {
  for (const path of ['../src/components/tabs/AiTab.vue', '../src/components/LibraryChat.vue']) {
    const source = await readFile(new URL(path, import.meta.url), 'utf8')
    const script = source.slice(source.indexOf('>') + 1, source.indexOf('</script>'))
    const ast = ts.createSourceFile(path, script, ts.ScriptTarget.Latest, true)
    const fn = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name.text === 'answerWrite').getText(ast)
    for (const approved of [true, false]) {
      const pending = { requestId: 'colored-request', preview: { passages: [{ color: '#FFEB3B' }] } }
      const queue = ref([pending, { requestId: 'next-request' }]), calls = []
      const answer = new Function('currentWrite', 'pendingWrites', 'invoke', ts.transpileModule(`${fn}\nreturn answerWrite;`, {
        compilerOptions: { target: ts.ScriptTarget.ES2021 },
      }).outputText)({ value: pending }, queue, async (command, args) => calls.push({ command, args }))
      answer(approved, ['#90CAF9'])
      assert.deepEqual(calls, [{ command: 'resolve_agent_write', args: { requestId: 'colored-request', approved, passageColors: approved ? ['#90CAF9'] : null } }])
      assert.equal(queue.value[0].requestId, 'next-request')
      assert.equal(pending.preview.passages[0].color, '#FFEB3B')
    }
  }
})
