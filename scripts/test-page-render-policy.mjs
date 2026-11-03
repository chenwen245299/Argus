import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const source = await readFile(new URL('../src/utils/pageRenderPolicy.ts', import.meta.url), 'utf8')
const { planPageRender, zonesFor, hiddenKeepSet, planHiddenBudget } = await import('data:text/javascript;base64,' + Buffer.from(ts.transpileModule(source, {
  compilerOptions:{ target:ts.ScriptTarget.ES2021, module:ts.ModuleKind.ESNext },
}).outputText).toString('base64'))
const geometry = { tops:Array.from({length:40}, (_, i) => i * 2012), heights:Array(40).fill(2000) }
const base = (extra = {}) => ({ geometry, scrollTop:10 * 2012, viewportHeight:900, direction:1, speed:0,
  rendered:new Set(), pagePixels:()=>9_000_000, budgetPixels:100_000_000, ...extra })

test('a zoomed book prepares two complete pages ahead, including when idle', () => {
  for (const direction of [0, 1]) {
    const p = planPageRender(base({ direction }))
    assert.deepEqual(p.render.slice(0, 3), [10, 11, 12])
  }
  const up = planPageRender(base({ direction:-1 }))
  assert.deepEqual(up.render.slice(0, 3), [10, 9, 8])
})
test('slow decodes get earlier look-ahead in the travel direction', () => {
  const fast = zonesFor(geometry, 20120, 900, 1, 6, {}, 20)
  const slow = zonesFor(geometry, 20120, 900, 1, 6, {}, 800)
  assert.ok(slow.zone.last > fast.zone.last)
  const up = zonesFor(geometry, 20120, 900, -1, 6, {}, 800)
  assert.ok(up.zone.first < fast.zone.first)
})
test('already painted pages survive leaving the keep zone while the budget has room', () => {
  const p = planPageRender(base({ rendered:new Set([0, 1, 2]) }))
  assert.deepEqual(p.evict, [])
  assert.ok(p.retainedPixels <= 100_000_000)
})
test('new look-ahead reserves space and evicts the least recently viewed bitmaps', () => {
  const rendered = new Set(Array.from({length:10}, (_, i) => i))
  const lastUsed = new Map([...rendered].map(i => [i, i === 0 ? 100 : i]))
  const p = planPageRender(base({ rendered, lastUsed }))
  assert.ok(p.evict.includes(1)); assert.ok(!p.evict.includes(0))
  assert.ok(p.retainedPixels <= 100_000_000)
  assert.ok(p.ready.includes(10) && p.ready.includes(12))
})
test('look-ahead itself is limited by the pixel budget at extreme zoom', () => {
  const p = planPageRender(base({ pagePixels:()=>35_000_000, speed:20, renderTimeMs:2000 }))
  assert.deepEqual(p.render, [10, 11])
  assert.equal(p.retainedPixels, 70_000_000)
})
test('visible pages and explicit jumps survive even when they exceed the budget', () => {
  const p = planPageRender(base({ pagePixels:()=>120_000_000, pinned:new Set([30]) }))
  assert.deepEqual(p.render, [30, 10])
  assert.deepEqual(p.evict, [])
})
test('a running page is cancelled outside the keep zone but never evicted underneath its renderer', () => {
  const p = planPageRender(base({ rendered:new Set([0, 10]), inFlight:new Set([0, 10]) }))
  assert.deepEqual(p.cancelInFlight, [0]); assert.deepEqual(p.evict, [])
  assert.ok(!p.render.includes(10))
})
test('obsolete zoom bitmaps outside the prepared pages are discarded', () => {
  const p = planPageRender(base({ rendered:new Set([0, 10]), stale:new Set([0, 10]) }))
  assert.deepEqual(p.evict, [0]); assert.equal(p.render[0], 10)
})
test('completed preparation is stable and does not queue the same pages again', () => {
  const first = planPageRender(base())
  const next = planPageRender(base({ rendered:new Set(first.ready) }))
  assert.deepEqual(next.render, []); assert.deepEqual(next.evict, [])
})
test('hidden tabs still retain the return viewport and obey their shared budget', () => {
  const keep = hiddenKeepSet(geometry, 20120, 900, 1)
  assert.ok(keep.has(10))
  assert.equal(hiddenKeepSet(geometry, 20120, 900, 3).size, 0)
  const levels = planHiddenBudget([
    {id:'old',hiddenSince:1,level:0,pixelsNow:90,pixelsAtLevel:[60,30,0]},
    {id:'new',hiddenSince:2,level:0,pixelsNow:90,pixelsAtLevel:[60,30,0]},
  ], 90)
  assert.equal(levels.get('old'), 2); assert.equal(levels.get('new'), 1)
})
