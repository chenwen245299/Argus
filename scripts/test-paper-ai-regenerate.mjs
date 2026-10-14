// Run: node --test scripts/test-paper-ai-regenerate.mjs. Local stream/provider mocks only.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { webcrypto } from 'node:crypto'
import { ref, shallowReactive } from 'vue'
import ts from 'typescript'

const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const { buildToolExchangeMessages } = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/agentHistory.ts', import.meta.url), 'utf8'),
)).toString('base64'))

// Execute the real regenerate -> streamAnswer -> agent-event -> save path.
// Only desktop I/O and markdown rendering are replaced; answer state and Vue
// proxies stay real, including the snapshot saved before the request begins.
const source = await readFile(new URL('../src/components/tabs/AiTab.vue', import.meta.url), 'utf8')
const ast = ts.createSourceFile('AiTab.ts', source.slice(source.indexOf('>') + 1, source.indexOf('</script>')), ts.ScriptTarget.Latest, true)
const functions = ['nowIso', 'findReactiveAnswer', 'isAnswerVisible', 'detachAnswerListeners',
  'activeAnswerForGroup', 'buildHistoryUntil', 'flushStreamRender', 'resetAnswerResult',
  'regenerate', 'streamAnswer', 'streamAnswerImpl', 'summarizeToolArgs', 'applyUsage', 'dismissWrite']
  .map(name => {
    const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name)
    assert.ok(node, `AiTab function ${name} exists`)
    return node.getText(ast)
  }).join('\n')
const eventSuffixes = ast.statements.find(n => ts.isVariableStatement(n) && n.declarationList.declarations
  .some(d => ts.isIdentifier(d.name) && d.name.text === 'ANSWER_EVENT_SUFFIXES')).getText(ast)
const makeRunner = new Function('env', transpile(`
  const streamTasks = new Map();
  const syncDisposed = false;
  const { props, activeConversation, activeAnswerTabs, expandedSteps, streamOwners,
    activeRequestIds, unlisteners, useReasoning, reasoningLevel, pendingWrites,
    streamRenderTimers, streamRenderLast, streamRenderCost, ai, reader, crypto,
    listen, invoke, persistConversationFor, scheduleStreamRender, scrollToBottom,
    mergeServerToolTrace, buildToolExchangeMessages, syncLibraryRoot,
    broadcastConversationSnapshot, broadcastStreamUpdate } = env;
  ${eventSuffixes}
  ${functions}
  return { regenerate };
`))

const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const image = file => ({ slug: 'paper', page: 4, file })
function oldAnswer(id) {
  return {
    id, providerId: 'provider', providerName: 'Provider', modelId: 'model', modelName: 'Model',
    content: 'Previous answer', displayContent: 'Previous painted answer',
    reasoningContent: 'Previous reasoning', displayReasoning: 'Previous painted reasoning',
    error: true, errorText: 'Previous error', streaming: false, createdAt: '2025-01-01',
    steps: [{ tool: 'view_paper_page', args: '', ok: true, preview: 'Old result', images: [image('old.png')] },
      { tool: 'get_paper_fulltext', args: '', ok: true, preview: 'Old text' }],
    serverErrors: ['old-server'], limitHit: { rounds: 50, max: 50 },
    serverTools: { oldTrace: true }, audioUrl: 'old-audio',
    tokenEstimate: 10, inputTokens: 20, outputTokens: 10, totalTokens: 30,
    cacheHitTokens: 15, costUsd: 1, startedAt: 1, endedAt: 2,
  }
}

function harness() {
  const previous = { ...oldAnswer('previous-answer'), error: false }
  const conv = {
    id: 'conversation', slug: 'paper', nodes: [
      { id: 'previous-question', role: 'user', content: 'Previous question' },
      { id: 'previous-group', role: 'assistantGroup', promptId: 'previous-question', answers: [previous] },
      { id: 'question', role: 'user', content: 'Current question' },
      { id: 'group', role: 'assistantGroup', promptId: 'question', answers: [oldAnswer('answer'), oldAnswer('other-answer')] },
    ],
  }
  const activeConversation = ref(conv)
  const group = activeConversation.value.nodes.at(-1)
  const answer = group.answers[0], other = group.answers[1]
  const listeners = new Map(), unlisteners = new Map(), snapshots = [], requests = []
  const expandedSteps = ref(new Set(['answer:0', 'answer:1', 'other-answer:0']))
  let ready = deferred()
  const env = {
    props: { slug: 'paper' }, activeConversation, activeAnswerTabs: ref({}), expandedSteps,
    streamOwners: shallowReactive(new Map()), activeRequestIds: new Map(), unlisteners,
    streamRenderTimers: new Map(), streamRenderLast: new Map(), streamRenderCost: new Map(),
    useReasoning: ref(true), reasoningLevel: ref('medium'), pendingWrites: ref([]), crypto: webcrypto,
    ai: { settings: { providers: [{ id: 'provider', base_url: 'https://example.test' }] } },
    reader: { readingStateFor: () => ({ page: 4 }) },
    syncLibraryRoot: 'library', broadcastConversationSnapshot: () => {}, broadcastStreamUpdate: () => {},
    listen: async (channel, callback) => {
      listeners.set(channel, callback)
      return () => listeners.delete(channel)
    },
    invoke: (command, args) => {
      assert.equal(command, 'chat_with_library')
      const response = deferred()
      requests.push({ args, response })
      ready.resolve()
      return response.promise
    },
    persistConversationFor: (_slug, conversation) => snapshots.push(JSON.parse(JSON.stringify(conversation))),
    scheduleStreamRender: a => { a.displayContent = a.content; a.displayReasoning = a.reasoningContent },
    scrollToBottom: () => {}, mergeServerToolTrace: (prior, next) => ({ ...prior, ...next }),
    buildToolExchangeMessages,
  }
  const runner = makeRunner(env)
  const begin = () => {
    ready = deferred()
    return { run: runner.regenerate(group, answer), ready: ready.promise }
  }
  const emit = (suffix, payload) => {
    const eventName = requests.at(-1).args.eventName
    const listener = listeners.get(eventName + suffix)
    assert.ok(listener, `Listener for ${suffix} is active`)
    listener({ payload })
  }
  return { ...env, answer, other, snapshots, requests, listeners, begin, emit }
}

function assertCleared(answer) {
  for (const field of ['content', 'displayContent', 'reasoningContent', 'displayReasoning', 'errorText']) {
    assert.equal(answer[field], '', `${field} is cleared`)
  }
  for (const field of ['steps', 'serverErrors', 'limitHit', 'serverTools', 'audioUrl',
    'tokenEstimate', 'inputTokens', 'outputTokens', 'totalTokens', 'cacheHitTokens', 'costUsd']) {
    assert.equal(answer[field], undefined, `${field} belongs to the new run`)
  }
  assert.equal(answer.error, false)
}

test('regenerate replaces the old tool trail, images and persisted result before new events', async () => {
  const h = harness()
  const untouched = JSON.stringify(h.other)
  const first = h.begin()
  assertCleared(h.answer)
  assertCleared(h.snapshots[0].nodes.at(-1).answers[0])
  assert.deepEqual([...h.expandedSteps.value], ['other-answer:0'])
  await first.ready
  assert.ok(h.answer.streaming)
  // Earlier turns still replay their tools; the replaced answer is excluded.
  assert.ok(h.requests[0].args.messages.some(m => m.role === 'tool' && m.content === 'Old result'))
  assert.ok(h.requests[0].args.messages.filter(m => m.role === 'tool').every(m => m.tool_call_id.startsWith('call_previous-answer_')))

  h.emit('-agent', { phase: 'tool', tool: 'view_paper_page', arguments: { pages: [5] } })
  h.emit('-agent', { phase: 'result', tool: 'view_paper_page', ok: true, preview: 'New result', images: [image('new.png')] })
  h.emit('-agent', { phase: 'servers', failed: ['new-server'] })
  h.emit('-agent', { phase: 'limit', rounds: 2, max: 2 })
  assert.equal(h.answer.steps.length, 1)
  assert.equal(h.answer.steps[0].preview, 'New result')
  assert.deepEqual(h.answer.steps[0].images.map(i => i.file), ['new.png'])
  h.requests[0].response.resolve('New answer')
  await first.run
  const saved = h.snapshots.at(-1).nodes.at(-1).answers[0]
  assert.equal(saved.steps.length, 1)
  assert.equal(saved.steps[0].images[0].file, 'new.png')
  assert.equal(saved.content, 'New answer')
  assert.equal(h.listeners.size, 0)
  assert.equal(JSON.stringify(h.other), untouched)

  // A subsequent run with no tools cannot display the preceding run's trail.
  const second = h.begin()
  assertCleared(h.answer)
  await second.ready
  h.requests[1].response.resolve('Plain new answer')
  await second.run
  assert.equal(h.answer.steps, undefined)
  assert.equal(h.answer.serverErrors, undefined)
  assert.equal(h.answer.limitHit, undefined)
  assert.equal(h.answer.content, 'Plain new answer')
  assert.equal(JSON.stringify(h.other), untouched)
})

test('a failed regenerated request keeps its own error without restoring the old tools', async () => {
  const h = harness()
  const { run, ready } = h.begin()
  await ready
  h.requests[0].response.reject(new Error('Current request failed'))
  await run
  assert.equal(h.answer.error, true)
  assert.match(h.answer.errorText, /Current request failed/)
  assert.equal(h.answer.steps, undefined)
  assert.equal(h.answer.streaming, false)
  assert.equal(h.listeners.size, 0)
  assert.equal(h.snapshots.at(-1).nodes.at(-1).answers[0].steps, undefined)
})

test('regenerate cancels only this answer\'s pending paint from the previous run', async t => {
  const h = harness()
  const timer = setTimeout(() => { h.answer.displayContent = 'Stale paint' }, 1000)
  const otherTimer = setTimeout(() => {}, 1000)
  t.after(() => { clearTimeout(timer); clearTimeout(otherTimer) })
  h.streamRenderTimers.set('answer', timer)
  h.streamRenderTimers.set('other-answer', otherTimer)
  h.streamRenderLast.set('answer', 10)
  h.streamRenderCost.set('answer', 20)
  const { run, ready } = h.begin()
  assert.equal(h.streamRenderTimers.has('answer'), false)
  assert.equal(h.streamRenderTimers.get('other-answer'), otherTimer)
  assert.equal(h.streamRenderLast.has('answer'), false)
  assert.equal(h.streamRenderCost.has('answer'), false)
  await ready
  h.requests[0].response.resolve('New answer')
  await run
  assert.equal(h.answer.displayContent, 'New answer')
})
