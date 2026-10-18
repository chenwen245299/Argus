// Run: node --test scripts/test-paper-ai-sync.mjs. Two real sync clients, local event/disk mocks.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { ref, nextTick, shallowReactive } from 'vue'
import { webcrypto } from 'node:crypto'
import ts from 'typescript'

const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const helper = transpile(await readFile(new URL('../src/utils/paperAiSync.ts', import.meta.url), 'utf8'))
  .replace(/from '@tauri-apps\/api\/event'/g, `from '${import.meta.resolve('@tauri-apps/api/event')}'`)
const { PaperAiSync, reconcilePaperAiConversations, translatePaperAiClocks, PAPER_AI_SYNC_INTERVAL_MS } =
  await import('data:text/javascript;base64,' + Buffer.from(helper).toString('base64'))
const { isGuidedReadingPrompt } = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/guidedReading.ts', import.meta.url), 'utf8'),
)).toString('base64'))
const { legacyChatRun } = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/chatRunStats.ts', import.meta.url), 'utf8'),
)).toString('base64'))

const source = await readFile(new URL('../src/components/tabs/AiTab.vue', import.meta.url), 'utf8')
const ast = ts.createSourceFile('AiTab.ts', source.slice(source.indexOf('>') + 1, source.indexOf('</script>')), ts.ScriptTarget.Latest, true)
const functions = [
  'nowIso', 'newId', 'cloneConversation', 'persistableSteps', 'persistableConversation',
  'createBlankConversation', 'normalizeConversations', 'persistedConversations', 'deletionTombstones',
  'saveConversationsToPaper', 'persistConversationFor', 'queueBackgroundSave', 'saveConversationForOtherPaper',
  'localConversationStreams', 'broadcastConversationSnapshot', 'broadcastStreamUpdate', 'ownsConversationStream',
  'adoptSyncedConversation', 'handlePaperAiSync', 'applySyncedDiskConversations', 'refreshSyncedConversations',
  'liveConversationById', 'restoreSession', 'openConversation', 'startNewConversation',
  'firstUserTitle', 'isMetadataExtractionConversation', 'stopConversationStreaming', 'conversationScope',
  'detachAnswerListeners', 'findReactiveAnswer', 'flushStreamRender', 'legacyPaperRuns',
].map(name => {
  const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name)
  assert.ok(node, `AiTab function ${name} exists`)
  return node.getText(ast)
}).join('\n')
const constants = ['PERSIST_STEP_CHARS', 'PERSIST_ANSWER_CHARS', 'ANSWER_EVENT_SUFFIXES'].map(name =>
  ast.statements.find(n => ts.isVariableStatement(n) && n.declarationList.declarations.some(d =>
    ts.isIdentifier(d.name) && d.name.text === name)).getText(ast)).join('\n')
const createView = new Function('env', transpile(`
  const { props, activeConversation, conversations, activeAnswerTabs, expandedSteps,
    streamOwners, activeRequestIds, unlisteners, pendingWrites, streamRenderTimers,
    streamRenderLast, streamRenderCost, paperSaves, backgroundSaves, sessions, knownConvIds,
    input, editingNodeId, showHistory, textareaEl, nextTick, scrollToBottom,
    invoke, paperAiSync, crypto, persistableServerToolTrace, saveLegacyActiveHistory,
    flattenConversation, reconcilePaperAiConversations, translatePaperAiClocks, isGuidedReadingPrompt, legacyChatRun, t, messageQueue } = env;
  let syncLibraryRoot = env.root, syncDisposed = false;
  let syncRefresh = null, syncRefreshPending = false;
  let selectionRevision = 0, initialSyncSelectionRevision = null;
  ${constants}
  ${functions}
  return {
    receive: handlePaperAiSync, broadcast: broadcastConversationSnapshot,
    update: broadcastStreamUpdate, persist: persistConversationFor,
    refresh: refreshSyncedConversations, restore: restoreSession,
    stop: stopConversationStreaming, normalize: normalizeConversations,
    choose: openConversation,
    initialRequest: () => { initialSyncSelectionRevision = selectionRevision; },
    dispose: () => { syncDisposed = true; },
  };
`))

const clone = value => JSON.parse(JSON.stringify(value))
const deferred = () => {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}
const conversation = (id = 'chat', content = 'Saved answer', time = '2026-01-01T00:00:00.000Z') => ({
  id, slug: 'paper', title: id, createdAt: time, updatedAt: time,
  nodes: [
    { id: `${id}-user`, role: 'user', content: 'Question', createdAt: time },
    { id: `${id}-group`, role: 'assistantGroup', promptId: `${id}-user`, createdAt: time,
      answers: [{ id: `${id}-answer`, content, createdAt: time, providerId: 'provider', providerName: 'Provider', modelId: 'model', modelName: 'Model', streaming: false }] },
  ],
})

function bus(initial = [conversation()]) {
  const handlers = new Set(), views = [], messages = [], writes = [], cancellations = []
  let disk = clone(initial)
  let dropFinal = false
  const transport = {
    emit: async (_name, payload) => {
      messages.push(clone(payload))
      if (dropFinal && payload.kind === 'snapshot' && payload.streams.length === 0) return
      for (const handler of handlers) handler(clone(payload))
    },
    listen: async (_name, handler) => { handlers.add(handler); return () => handlers.delete(handler) },
  }
  async function view(origin, root = 'root-a', slug = 'paper') {
    let client
    const pausedScopes = new Set()
    const sync = {
      get origin() { return client.origin },
      send: p => client.send(p), queue: (id, make) => client.queue(id, make), cancel: id => client.cancel(id),
    }
    const current = disk.filter(c => c.slug === slug)
    const env = {
      props: { slug }, root, activeConversation: ref(current[0] ? clone(current[0]) : null),
      conversations: ref(clone(current)), activeAnswerTabs: ref({}), expandedSteps: ref(new Set()),
      streamOwners: shallowReactive(new Map()), activeRequestIds: new Map(), unlisteners: new Map(),
      pendingWrites: ref([]), streamRenderTimers: new Map(), streamRenderLast: new Map(), streamRenderCost: new Map(),
      paperSaves: new Map(), backgroundSaves: new Map(), sessions: new Map(), knownConvIds: new Map([[slug, new Set(current.map(c => c.id))]]),
      input: ref(''), editingNodeId: ref(null), showHistory: ref(false), textareaEl: ref(null),
      crypto: webcrypto, nextTick, scrollToBottom: () => {}, paperAiSync: sync,
      pausedScopes, messageQueue: { pause: scope => pausedScopes.add(scope) },
      persistableServerToolTrace: trace => trace, saveLegacyActiveHistory: async () => {}, flattenConversation: () => [],
      reconcilePaperAiConversations, translatePaperAiClocks, isGuidedReadingPrompt, legacyChatRun,
      t: key => key === 'guidedReading.title' ? 'AI 领读' : key,
    }
    let readGate = null
    env.invoke = async (command, args) => {
      if (command === 'get_paper_ai_conversations') {
        const snapshot = clone(disk.filter(c => c.slug === args.slug))
        if (readGate) { const gate = readGate; readGate = null; await gate.promise }
        return snapshot
      }
      if (command === 'save_paper_ai_conversations') {
        assert.equal(args.origin, origin)
        assert.equal(args.expectedRoot, root)
        const next = new Map(disk.map(c => [c.id, c]))
        for (const conv of args.conversations) {
          if (!next.has(conv.id) || next.get(conv.id).updatedAt < conv.updatedAt) next.set(conv.id, clone(conv))
        }
        for (const t of args.tombstones ?? []) if ((next.get(t.id)?.updatedAt ?? '') <= t.deleted_at) next.delete(t.id)
        disk = [...next.values()]
        writes.push({ origin, ...clone(args) })
        // This is the Rust command's post-commit notification, not a polling timer.
        for (const v of views) if (v.client.origin !== origin && v.root === root && v.props.slug === args.slug) void v.refresh()
        return
      }
      if (command === 'cancel_ai_request') { cancellations.push(args.requestId); return }
      if (command === 'save_chat_history') return
      throw new Error(`Unexpected command ${command}`)
    }
    const methods = createView(env)
    client = new PaperAiSync(methods.receive, transport, origin)
    await client.start()
    const h = { ...env, ...methods, client, delayRead: () => { readGate = deferred(); return readGate },
      own: (conv = env.activeConversation.value) => {
        const answer = conv.nodes.at(-1).answers[0]
        answer.streaming = true
        answer.createdAt = '2026-10-12T01:00:00.000Z'
        answer.startedAt = performance.now()
        answer.endedAt = undefined
        env.streamOwners.set(answer.id, { slug: conv.slug, conv, root })
        env.activeRequestIds.set(answer.id, `${origin}-request`)
        return answer
      },
    }
    views.push(h)
    return h
  }
  return { view, messages, writes, cancellations, handlers,
    get disk() { return disk }, set dropFinal(value) { dropFinal = value },
    dispose: () => views.forEach(v => { v.dispose(); v.client.dispose() }),
  }
}

test('saved edits and deletes synchronize both ways without resetting the draft or selected conversation', async t => {
  const b = bus([conversation(), conversation('other')]); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  popup.input.value = 'Unsent question'
  popup.activeAnswerTabs.value = { tab: 'chosen-model' }
  sidebar.activeConversation.value.nodes.at(-1).answers[0].content = 'Changed in sidebar'
  sidebar.persist('paper', sidebar.activeConversation.value)
  await sidebar.paperSaves.get('paper')
  await popup.refresh()
  assert.equal(popup.activeConversation.value.id, 'chat')
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Changed in sidebar')
  assert.equal(popup.input.value, 'Unsent question')
  assert.deepEqual(popup.activeAnswerTabs.value, { tab: 'chosen-model' })
  popup.activeConversation.value.nodes.at(-1).answers[0].content = 'Changed in popup'
  popup.persist('paper', popup.activeConversation.value)
  await popup.paperSaves.get('paper'); await sidebar.refresh()
  assert.equal(sidebar.activeConversation.value.nodes.at(-1).answers[0].content, 'Changed in popup')
  assert.equal(b.writes.length, 2, 'receivers never echo saves back to disk')
  popup.conversations.value = popup.conversations.value.filter(c => c.id !== 'other')
  // The real save path computes the deletion tombstone from its synced baseline.
  popup.persist('paper', popup.activeConversation.value)
  await popup.paperSaves.get('paper'); await sidebar.refresh()
  assert.deepEqual(sidebar.conversations.value.map(c => c.id), ['chat'])
})

test('a popup opened during generation joins the current answer and sees bounded live updates', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar')
  const answer = sidebar.own()
  answer.content = 'First part'
  answer.steps = [{ tool: 'view_paper_page', args: '', ok: true, images: [{ slug: 'paper', page: 4, file: 'page.png', dataUrl: 'large-live-image' }] }]
  const popup = await b.view('popup')
  popup.initialRequest()
  await popup.client.send({ kind: 'request', root: 'root-a', slug: 'paper', select: true })
  const shown = popup.activeConversation.value.nodes.at(-1).answers[0]
  assert.equal(shown.content, 'First part')
  assert.equal(shown.streaming, true)
  assert.equal(shown.steps[0].images[0].file, 'page.png')
  assert.equal(shown.steps[0].images[0].dataUrl, undefined)
  answer.content = 'Second part'; sidebar.update(answer.id)
  answer.content = 'Latest part'; sidebar.update(answer.id)
  assert.equal(b.messages.filter(p => p.kind === 'update').length, 0)
  t.mock.timers.tick(PAPER_AI_SYNC_INTERVAL_MS)
  assert.equal(b.messages.filter(p => p.kind === 'update').length, 1)
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Latest part')
  assert.equal(b.writes.length, 0, 'streaming is synchronized without repeated file writes')
})

test('a delayed disk read cannot overwrite a newer live answer; durable completion repairs a missed final event', async t => {
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  const gate = popup.delayRead()
  const refreshing = popup.refresh()
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve()
  const answer = sidebar.own()
  answer.content = 'Live answer'
  sidebar.broadcast(sidebar.activeConversation.value)
  gate.resolve(); await refreshing
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Live answer')
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].streaming, true)
  b.dropFinal = true
  answer.content = 'Completed answer'; answer.streaming = false; answer.endedAt = performance.now()
  sidebar.streamOwners.delete(answer.id)
  sidebar.persist('paper', sidebar.activeConversation.value)
  await sidebar.paperSaves.get('paper'); await popup.refresh()
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Completed answer')
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].streaming, false)
  assert.equal(popup.streamOwners.size, 0)
})

test('a missed initial snapshot is repaired when an unknown answer update arrives', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  popup.conversations.value = []
  popup.activeConversation.value = null
  popup.initialRequest()
  const answer = sidebar.own(); answer.content = 'Already generating'
  sidebar.update(answer.id)
  t.mock.timers.tick(PAPER_AI_SYNC_INTERVAL_MS)
  assert.equal(popup.activeConversation.value.id, 'chat')
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Already generating')
  assert.ok(popup.activeConversation.value.nodes.at(-1).answers[0].streaming)
})

test('a late join response does not take away a conversation selected by the user', async t => {
  const b = bus([conversation(), conversation('other')]); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  popup.initialRequest()
  popup.choose('other')
  popup.input.value = 'Draft for other conversation'
  await popup.client.send({ kind: 'request', root: 'root-a', slug: 'paper', select: true })
  assert.equal(popup.activeConversation.value.id, 'other')
  assert.equal(popup.input.value, 'Draft for other conversation')
})

test('rapid saves have strictly increasing timestamps and retain the latest model result', async t => {
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  const conv = sidebar.activeConversation.value
  conv.nodes.at(-1).answers[0].content = 'First model finished'
  sidebar.persist('paper', conv)
  const firstTime = conv.updatedAt
  conv.nodes.at(-1).answers[0].content = 'Second model finished'
  sidebar.persist('paper', conv)
  assert.ok(conv.updatedAt > firstTime)
  await sidebar.paperSaves.get('paper'); await popup.refresh()
  assert.equal(b.disk.find(c => c.id === 'chat').nodes.at(-1).answers[0].content, 'Second model finished')
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].content, 'Second model finished')
})

test('a mirrored stop reaches the owning request instead of saving the receiver\'s stale copy', async t => {
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  sidebar.own(); sidebar.broadcast(sidebar.activeConversation.value)
  popup.stop(popup.activeConversation.value)
  await sidebar.paperSaves.get('paper')
  assert.deepEqual(b.cancellations, ['sidebar-request'])
  assert.ok(b.writes.every(w => w.origin === 'sidebar'))
  assert.equal(popup.activeConversation.value.nodes.at(-1).answers[0].streaming, false)
  assert.deepEqual([...sidebar.pausedScopes], [JSON.stringify(['root-a', 'paper', 'chat'])])
})

test('sync isolates papers/libraries and preserves the local object that owns a stream', async t => {
  const b = bus(); t.after(b.dispose)
  const source = await b.view('source'), same = await b.view('same'), otherRoot = await b.view('other-root', 'root-b')
  const otherPaper = await b.view('other-paper', 'root-a', 'different-paper')
  const localAnswer = same.own(); const localObject = same.activeConversation.value
  const answer = source.own(); answer.content = 'Foreign stream'; source.broadcast(source.activeConversation.value)
  assert.equal(same.activeConversation.value, localObject)
  assert.equal(same.activeConversation.value.nodes.at(-1).answers[0], localAnswer)
  assert.equal(localAnswer.content, 'Saved answer')
  assert.equal(otherRoot.activeConversation.value.nodes.at(-1).answers[0].content, 'Saved answer')
  assert.equal(otherPaper.conversations.value.length, 0)
})

test('returning to a paper restores fresh messages while keeping the cached input and model tab', async t => {
  const b = bus(); t.after(b.dispose)
  const main = await b.view('main'), popup = await b.view('popup')
  main.sessions.set('paper', { conv: clone(main.activeConversation.value), input: 'Cached draft', tabs: { group: 'model' } })
  main.activeConversation.value = null
  popup.activeConversation.value.nodes.at(-1).answers[0].content = 'Fresh persisted messages'
  popup.persist('paper', popup.activeConversation.value)
  await popup.paperSaves.get('paper'); await main.refresh()
  main.restore('paper')
  assert.equal(main.activeConversation.value.nodes.at(-1).answers[0].content, 'Fresh persisted messages')
  assert.equal(main.input.value, 'Cached draft')
  assert.deepEqual(main.activeAnswerTabs.value, { group: 'model' })
})

test('older live events are rejected and disposal releases even a delayed subscription', async () => {
  const listeners = [], received = []
  const started = deferred()
  let unlistened = 0
  const transport = { emit: async () => {}, listen: async (_name, handler) => { listeners.push(handler); await started.promise; return () => { unlistened++ } } }
  const client = new PaperAiSync(p => received.push(p), transport, 'receiver')
  const opening = client.start()
  const base = { kind: 'update', root: 'root', slug: 'paper', conversationId: 'chat', answers: [], streams: [], origin: 'sender', clock: 1 }
  listeners[0]({ ...base, revision: 2 })
  listeners[0]({ ...base, revision: 1 })
  listeners[0]({ ...base, revision: 3, target: 'someone-else' })
  assert.deepEqual(received.map(p => p.revision), [2])
  client.dispose(); started.resolve(); await opening
  assert.equal(unlistened, 1)
})

test('disk reconciliation removes deleted records but keeps protected streams; clocks retain elapsed durations', () => {
  const active = conversation('active', 'Partial', '2026-10-12T01:00:00.000Z')
  const old = conversation('active', 'Old', '2026-01-01T00:00:00.000Z')
  const list = reconcilePaperAiConversations([active, conversation('deleted')], [old], new Set(['active']))
  assert.deepEqual(list.map(c => c.id), ['active'])
  assert.equal(list[0], active)
  const answers = [{ startedAt: 20, endedAt: 120 }, { startedAt: 20 }]
  translatePaperAiClocks(answers, 150)
  assert.equal(answers[0].endedAt - answers[0].startedAt, 100)
  assert.ok(Math.abs(performance.now() - answers[1].startedAt - 130) < 10)
})

test('context compaction joins across windows, forwards stop, and repairs a missed final event', async t => {
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  const conv = sidebar.activeConversation.value
  conv.contextTask = 'context-task'
  sidebar.streamOwners.set('context-task', { slug: 'paper', conv, root: 'root-a', contextOnly: true })
  sidebar.activeRequestIds.set('context-task', 'context-request')
  sidebar.persist('paper', conv)
  await sidebar.paperSaves.get('paper')
  assert.equal(popup.streamOwners.get('context-task')?.contextOnly, true)
  popup.stop(popup.activeConversation.value)
  await sidebar.paperSaves.get('paper')
  assert.deepEqual(b.cancellations, ['context-request'])
  assert.equal(popup.streamOwners.has('context-task'), false)

  conv.contextTask = 'next-context-task'
  sidebar.streamOwners.set('next-context-task', { slug: 'paper', conv, root: 'root-a', contextOnly: true })
  sidebar.persist('paper', conv)
  await sidebar.paperSaves.get('paper')
  assert.equal(popup.streamOwners.has('next-context-task'), true)
  b.dropFinal = true
  delete conv.contextTask
  conv.contextStates = { 'provider::model': { checkpoint: { throughMessageId: 'chat-group', fingerprint: 'saved', messages: [{ role: 'user', content: 'Summary' }] } } }
  sidebar.streamOwners.delete('next-context-task')
  sidebar.persist('paper', conv)
  await sidebar.paperSaves.get('paper'); await popup.refresh()
  assert.equal(popup.streamOwners.has('next-context-task'), false)
  assert.equal(popup.activeConversation.value.contextStates['provider::model'].checkpoint.messages[0].content, 'Summary')
})

test('live usage patches preserve per-model checkpoints without resending native history', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  const b = bus(); t.after(b.dispose)
  const sidebar = await b.view('sidebar'), popup = await b.view('popup')
  const conv = sidebar.activeConversation.value
  conv.contextStates = { 'provider::model': { checkpoint: { throughMessageId: 'chat-user', fingerprint: 'saved', messages: [{ role: 'user', content: 'Summary' }] }, usage: { usedTokens: 1000 } } }
  const answer = sidebar.own()
  sidebar.broadcast(conv)
  conv.contextStates['provider::model'].usage = { usedTokens: 4000 }
  conv.usageRuns = {
    current: {id:'current',answerId:answer.id,providerId:'provider',modelId:'model',inputTokens:1000,outputTokens:200,modelCalls:2},
    earlier: {id:'earlier',answerId:'old-answer',providerId:'provider',modelId:'model',inputTokens:100},
  }
  sidebar.update(answer.id); t.mock.timers.tick(PAPER_AI_SYNC_INTERVAL_MS)
  const update = b.messages.filter(p => p.kind === 'update').at(-1)
  assert.equal(update.contextUsage['provider::model'].usedTokens, 4000)
  assert.deepEqual(Object.keys(update.usageRuns), ['current'], 'only current accounting travels in live patches')
  assert.equal(popup.activeConversation.value.usageRuns.current.inputTokens, 1000)
  assert(!JSON.stringify(update).includes('Summary'))
  assert.equal(popup.activeConversation.value.contextStates['provider::model'].usage.usedTokens, 4000)
  assert.equal(popup.activeConversation.value.contextStates['provider::model'].checkpoint.messages[0].content, 'Summary')
})
