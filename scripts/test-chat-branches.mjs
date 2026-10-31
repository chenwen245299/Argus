import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const source = await readFile(new URL('../src/utils/chatBranches.ts', import.meta.url), 'utf8')
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext } }).outputText
const { conversationBranchRows, cloneBranchRecords, branchImageFiles } = await import('data:text/javascript;base64,' + Buffer.from(compiled).toString('base64'))
const conv = (id, day, parentId) => ({ id, updatedAt: `2026-10-${day}`, ...(parentId ? { branch: { parentId } } : {}) })

test('a recently active nested branch brings its whole family forward, preserving ancestry', () => {
  const records = [conv('other', '30'), conv('child', '20', 'parent'), conv('parent', '10'), conv('grandchild', '31', 'child'), conv('sibling', '21', 'parent')]
  assert.deepEqual(conversationBranchRows(records).map(r => [r.conversation.id, r.depth]), [
    ['parent', 0], ['child', 1], ['grandchild', 2], ['sibling', 1], ['other', 0],
  ])
  assert.deepEqual(conversationBranchRows(records).map(r => r.branchCount), [3, 1, 0, 0, 0])
  assert.deepEqual(records.map(r => r.id), ['other', 'child', 'parent', 'grandchild', 'sibling'])
})

test('deleted parents, self links and cyclic legacy data do not hide conversations', () => {
  const records = [conv('orphan', '31', 'deleted'), conv('self', '30', 'self'), conv('a', '29', 'b'), conv('b', '28', 'a')]
  const rows = conversationBranchRows(records)
  assert.equal(rows.length, 4)
  assert.equal(new Set(rows.map(r => r.conversation.id)).size, 4)
  assert.equal(rows.find(r => r.conversation.id === 'orphan').depth, 0)
})

test('fork snapshots detach attachments and full evidence while discarding live render state', () => {
  const original = [{ id: 'user', attachments: [{ name: 'notes.pdf', dataUrl: 'original attachment' }] }, {
    content: 'Complete answer', displayContent: 'stale rendered answer', streaming: true,
    reasoningContent: 'Preserved reasoning', displayReasoning: 'stale', audioUrl: 'live-only',
    contextContent: 'volatile', agentSteps: [{ ok: true, preview: 'x'.repeat(200000), argsJson: '{"page":2}',
      images: [{ file: 'figure.png', crop: { x: 0.2 } }, { file: 'figure.png' }] }],
  }]
  const fork = cloneBranchRecords(original)
  assert.equal(fork[1].streaming, false)
  assert.equal(fork[1].displayContent, undefined)
  assert.equal(fork[1].displayReasoning, undefined)
  assert.equal(fork[1].audioUrl, undefined)
  assert.equal(fork[1].contextContent, undefined)
  assert.equal(fork[1].agentSteps[0].preview.length, 200000)
  assert.deepEqual(branchImageFiles(fork), ['figure.png'])
  fork[0].attachments[0].name = 'Changed independently'
  fork[1].agentSteps[0].images[0].crop.x = 0.8
  assert.equal(original[0].attachments[0].name, 'notes.pdf')
  assert.equal(original[1].agentSteps[0].images[0].crop.x, 0.2)
  assert.equal(original[1].streaming, true)
})
