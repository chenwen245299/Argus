// Real venue classification/badges and Pinia cache, with local IPC fixtures.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { defineStore, createPinia } from 'pinia'
import { ref } from 'vue'
import ts from 'typescript'

const helperSource = await readFile(new URL('../src/utils/rankBadges.ts', import.meta.url), 'utf8')
const helperCode = ts.transpileModule(helperSource, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const { isUnpublishedVenue, badgesFromRank } = await import(`data:text/javascript;base64,${Buffer.from(helperCode).toString('base64')}`)

const storeSource = await readFile(new URL('../src/stores/ranks.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('ranks.ts', storeSource, ts.ScriptTarget.Latest, true)
const storeCode = ts.transpileModule(ast.statements.filter(s => !ts.isImportDeclaration(s)).map(s => s.getText(ast)).join('\n'), {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText.replace(/^export /gm, '')
const makeStore = new Function('defineStore', 'ref', 'invoke', 'isUnpublishedVenue', `${storeCode}\nreturn { useRanksStore, normVenue }`)

const unpublished = [
  'ICLR 2027 (under review)', 'ICLR 2027 Conference Submission',
  'Submitted to NeurIPS 2026', 'ICLR.cc/2027/Conference/Submission',
  'ICLR.cc/2025/Conference/Withdrawn_Submission', 'ICLR 2027 Under_Review',
  'ICLR 2027（under review中）',
  'TMLR under-review', 'ICLR 2025 Withdrawn Submission', 'ICLR 2025 (rejected)',
  'ICLR 2027 (在审)', 'ICML 2026 投稿中', 'TMLR 审稿中', 'ICLR 2027 未录用',
  'arXiv', 'bioRxiv preprint', 'ICLR 2027 (acceptance unconfirmed)',
]
const accepted = [
  'ICLR 2025', 'ICLR 2025 (accepted)', 'ICLR 2025 Poster', 'NeurIPS 2024',
  'International Conference on Learning Representations', 'Physical Review Letters',
  'IEEE Transactions on Pattern Analysis and Machine Intelligence',
]
const rank = venue => ({ official: { ccf: 'A' }, custom: [], venue, fetched_at: '' })

test('submission status blocks publication ranks without mistaking real venue names', () => {
  for (const venue of unpublished) {
    assert.equal(isUnpublishedVenue(venue), true, venue)
    assert.deepEqual(badgesFromRank(rank(venue)), [], venue)
  }
  for (const venue of accepted) {
    assert.equal(isUnpublishedVenue(venue), false, venue)
    assert.deepEqual(badgesFromRank(rank(venue)), [{ text: 'CCF A', color: '#e11d48' }], venue)
  }
  assert.equal(isUnpublishedVenue(undefined), false)
  assert.deepEqual(badgesFromRank(null), [])
})

function fixture() {
  const calls = [], cache = {}
  const invoke = async (command, args) => {
    calls.push({ command, args })
    if (command === 'get_venue_ranks') return cache
    assert.equal(command, 'fetch_journal_rank')
    return rank(args.venue)
  }
  const { useRanksStore, normVenue } = makeStore(defineStore, ref, invoke, isUnpublishedVenue)
  const store = useRanksStore(createPinia())
  return { store, normVenue, calls, cache }
}

test('old cached CCF results never escape for unpublished papers, even on a forced lookup', async () => {
  const { store, normVenue, calls, cache } = fixture()
  // An old cache may even have an unqualified venue inside the stored rank.
  for (const venue of unpublished) cache[normVenue(venue)] = rank('ICLR 2027')
  cache[normVenue('ICLR 2025')] = rank('ICLR 2025')
  await store.load()
  for (const venue of unpublished) {
    assert.equal(store.get(venue), undefined, venue)
    assert.equal(store.has(venue), false, venue)
    assert.deepEqual((await store.fetchRank(venue, true)).official, {}, venue)
  }
  assert.deepEqual(badgesFromRank(store.get('ICLR 2025')), [{ text: 'CCF A', color: '#e11d48' }])
  assert.deepEqual(calls.map(c => c.command), ['get_venue_ranks'])
  store.$dispose()
})

test('accepted venues still fetch and share their cache across years', async () => {
  const { store, calls } = fixture()
  const result = await store.fetchRank('ICLR 2025')
  assert.equal(result.official.ccf, 'A')
  assert.deepEqual(store.get('ICLR 2026'), result)
  assert.deepEqual(await store.fetchRank('ICLR 2026'), result)
  assert.equal(calls.length, 1)
  await store.fetchRank('ICLR 2026', true)
  assert.equal(calls.length, 2)
  store.$dispose()
})
