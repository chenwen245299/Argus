import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const source = await readFile(new URL('../src/utils/chatRunStats.ts', import.meta.url), 'utf8')
const api = {}
new Function('exports', ts.transpileModule(source, { compilerOptions: {
  target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS,
} }).outputText)(api)
const sample = id => ({id,answerId:'answer',providerId:'provider',modelId:'model'})

test('cumulative live samples replace themselves and regeneration retains paid usage after reload', () => {
  let owner = {}
  let run = api.beginChatRun(owner,sample('first'))
  api.applyRunUsage(run,{input_tokens:100,output_tokens:20,total_tokens:120,model_calls:1})
  api.applyRunUsage(run,{input_tokens:300,output_tokens:50,total_tokens:350,model_calls:2})
  owner = JSON.parse(JSON.stringify(owner))
  run = api.beginChatRun(owner,sample('regenerate'))
  api.applyRunUsage(run,{input_tokens:120,output_tokens:30,total_tokens:150,model_calls:1})
  const result = api.summarizeChatRuns(Object.values(owner.usageRuns))
  assert.equal(result.total,500);assert.equal(result.calls,3)
})
test('cache percentages are weighted and unknown samples hide a misleading rate', () => {
  const owner = {}
  const first = api.beginChatRun(owner,sample('a')),second = api.beginChatRun(owner,sample('b'))
  api.applyRunUsage(first,{input_tokens:100,output_tokens:10,cache_hit_tokens:100,cache_reported:true})
  api.applyRunUsage(second,{input_tokens:900,output_tokens:10,cache_hit_tokens:0,cache_reported:true})
  assert.equal(api.summarizeChatRuns(Object.values(owner.usageRuns)).cachePercent,10)
  api.applyRunUsage(second,{input_tokens:900,cache_hit_tokens:0,cache_reported:false})
  assert.equal(api.summarizeChatRuns(Object.values(owner.usageRuns)).cachePercent,null)
})
test('compaction batches replace their own usage and speed excludes their work and tool wait', () => {
  const owner = {}
  const run = api.beginChatRun(owner,{...sample('answer'),durationMs:60000})
  api.applyRunUsage(run,{input_tokens:100,output_tokens:200,model_duration_ms:2000,model_calls:2})
  for(const id of ['batch-a','batch-a','batch-b']) api.recordCompactionUsage(owner,{id,input_tokens:1000,output_tokens:50,model_calls:1},'p','m')
  const result = api.summarizeChatRuns(Object.values(owner.usageRuns))
  assert.equal(result.total,2400);assert.equal(result.speed,100);assert.equal(result.calls,4)
})
test('legacy data is seeded once and missing step counts remain an explicit lower bound', () => {
  const owner = {}, legacy = api.legacyChatRun({id:'old',inputTokens:200,outputTokens:10,cacheHitTokens:0},'p','m',5)
  api.beginChatRun(owner,sample('a'),[legacy]);api.beginChatRun(owner,sample('b'),[legacy])
  const result = api.summarizeChatRuns(Object.values(owner.usageRuns))
  assert.equal(result.total,210);assert.equal(result.calls,1);assert(result.callsEstimated)
  assert.equal(result.cachePercent,null)
})

test('billed costs replace cumulative samples and include regeneration and compaction exactly once after reload', () => {
  let owner = {}
  const first = api.beginChatRun(owner,sample('paid'))
  api.applyRunUsage(first,{input_tokens:100,output_tokens:20,cost_usd:0.1})
  api.applyRunUsage(first,{input_tokens:300,output_tokens:50,cost_usd:0.3})
  const next = api.beginChatRun(owner,sample('regenerated'))
  api.applyRunUsage(next,{input_tokens:120,output_tokens:30,cost_usd:0.2})
  api.recordCompactionUsage(owner,{id:'batch',input_tokens:500,output_tokens:20,cost_usd:0.05},'p','m')
  api.recordCompactionUsage(owner,{id:'batch',input_tokens:600,output_tokens:30,cost_usd:0.1},'p','m')
  owner = JSON.parse(JSON.stringify(owner))
  const cost = api.summarizeChatCost(Object.values(owner.usageRuns),7.2,()=>assert.fail('A real bill must win'))
  assert(Math.abs(cost.cny-4.32)<1e-9);assert.equal(cost.estimated,false)
})
test('free, unreported, estimated and missing costs remain distinct', () => {
  const run = sample('free')
  api.applyRunUsage(run,{input_tokens:100,output_tokens:20,cost_usd:0})
  assert.equal(api.summarizeChatCost([run],7.2,()=>99).cny,0)
  api.applyRunUsage(run,{input_tokens:100,output_tokens:20,cost_usd:null})
  assert.equal(api.summarizeChatCost([run],7.2,()=>0.25).cny,0.25)
  assert.equal(api.summarizeChatCost([run],7.2,()=>0.25).estimated,true)
  const missing = api.summarizeChatCost([run],7.2,()=>null)
  assert.equal(missing.cny,null);assert.equal(missing.missing,1)
  assert.equal(api.summarizeChatCost([sample('waiting')],7.2,()=>null).cny,0)
})
test('older visible bills repair only the latest request sharing an answer id', () => {
  const owner={usageRuns:{old:{...sample('old'),inputTokens:100,outputTokens:10},latest:{...sample('latest'),inputTokens:200,outputTokens:20}}}
  const legacy=api.legacyChatRun({id:'answer',costUsd:0.4,inputTokens:200,outputTokens:20},'provider','model')
  const runs=api.conversationChatRuns(owner,[legacy])
  assert.equal(runs[0].costUsd,undefined);assert.equal(runs[1].costUsd,0.4)
  assert.equal(owner.usageRuns.latest.costUsd,undefined)
  assert.equal(api.conversationChatRuns({},[legacy])[0].costUsd,0.4)
})
test('new runs retain pricing dates and small or zero bills have readable precision', () => {
  const run=api.beginChatRun({},sample('dated'))
  assert(Number.isFinite(Date.parse(run.createdAt)))
  assert.equal(api.formatChatCost(0),'0.00')
  assert.equal(api.formatChatCost(0.00001),'<0.0001')
  assert.equal(api.formatChatCost(0.0042),'0.0042')
  assert.equal(api.formatChatCost(1.23),'1.23')
})
test('legacy bill repair does not depend on persisted UUID order or guess between identical regenerations', () => {
  const latest={...sample('a-uuid'),inputTokens:200,outputTokens:20},old={...sample('z-uuid'),inputTokens:100,outputTokens:10}
  const bill=api.legacyChatRun({id:'answer',inputTokens:200,outputTokens:20,costUsd:0.4},'provider','model')
  const runs=api.conversationChatRuns({usageRuns:{a:latest,z:old}},[bill])
  assert.equal(runs[0].costUsd,0.4);assert.equal(runs[1].costUsd,undefined)
  const ambiguous=api.conversationChatRuns({usageRuns:{a:latest,z:{...latest,id:'another'}}},[bill])
  assert(ambiguous.every(run=>run.costUsd===undefined))
})
