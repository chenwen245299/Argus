import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const source = await readFile(new URL('../src/utils/chatContext.ts', import.meta.url), 'utf8')
const api = {}
new Function('exports', ts.transpileModule(source, { compilerOptions: {
  target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS,
} }).outputText)(api)
const history = rows => rows.map(({ role, content, tool_calls, tool_call_id, name }) =>
  ({ role, content, tool_calls, tool_call_id, name }))
const records = () => [{ id: 'u1', role: 'user', content: 'Original request' },
  { id: 'a1', role: 'assistant', content: 'Evidence' }, { id: 'u2', role: 'user', content: 'Follow-up' }]
const checkpoint = rows => ({ throughMessageId: 'a1', fingerprint: api.historyFingerprint(history(rows.slice(0, 2))),
  messages: [{ role: 'user', content: '<compacted-summary>Original intent and evidence</compacted-summary>' }] })

test('checkpoint survives serialization and appends new turns without deleting visible history', () => {
  const rows = records(), state = JSON.parse(JSON.stringify(checkpoint(rows)))
  const outgoing = api.applyContextCheckpoint(rows, state, history)
  assert.equal(outgoing.length, 2)
  assert.match(outgoing[0].content, /compacted-summary/)
  assert.equal(outgoing[1].content, 'Follow-up')
  assert.equal(rows[0].content, 'Original request')
})
test('edits, deletion, model answer changes, and regenerating before a checkpoint reject stale summaries', () => {
  for (const mutate of [r => r[0].content = 'Correction', r => r[1].content = 'Other model answer',
    r => r.splice(0, 1), r => r.splice(1)]) {
    const rows = records(), state = checkpoint(rows); mutate(rows)
    assert.deepEqual(api.applyContextCheckpoint(rows, state, history), history(rows))
  }
})
test('tool arguments/results and attachment changes invalidate a checkpoint', () => {
  for (const field of ['tool_calls', 'content']) {
    const rows = records(); rows[1].tool_calls = [{ id: 'call', function: { arguments: '{}' } }]
    const state = checkpoint(rows)
    rows[1][field] = field === 'content' ? [{ type: 'image_url', image_url: { url: 'data:changed' } }] : [{ id: 'other' }]
    assert.deepEqual(api.applyContextCheckpoint(rows, state, history), history(rows))
  }
})
test('persistence caps can validate without shortening the uncompressed tail', () => {
  const rows = records(), clipped = r => history(r).map(m => ({ ...m, content: m.content.slice(0, 5) }))
  const state = checkpoint(rows); state.fingerprint = api.historyFingerprint(clipped(rows.slice(0, 2)))
  const outgoing = api.applyContextCheckpoint(rows, state, history, clipped)
  assert.equal(outgoing.at(-1).content, 'Follow-up')
})
test('CJK, surrogate pairs and bounded context display', () => {
  assert.equal(api.estimateContextText('abcdefgh中文🦀'), 5)
  assert.equal(api.contextPercent({ usedTokens: 1200, contextWindow: 1000 }), 100)
  assert.equal(api.contextPercent({ usedTokens: 0, contextWindow: 1000 }), 0)
})
