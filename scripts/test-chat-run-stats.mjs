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
