// Local queue races and lifetime checks; no provider or desktop required.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import * as vue from 'vue'
import ts from 'typescript'

const source = await readFile(new URL('../src/utils/chatSendQueue.ts', import.meta.url), 'utf8')
const exports = {}
new Function('require', 'exports', ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS },
}).outputText)(() => vue, exports)
const deferred = () => { let resolve; const promise = new Promise(done => resolve = done); return { promise, resolve } }
const turn = async () => { for (let i = 0; i < 8; i++) { await Promise.resolve(); await vue.nextTick() } }
const message = (id, scope = 'a') => ({ id, scope, text: id, files: [{ id: 'file', name: 'paper.pdf' }] })
function harness() {
  const scope = vue.effectScope()
  const busy = vue.shallowReactive(new Set())
  const requests = [], interrupts = []
  let allowed = true, interrupt = async () => {}
  const queue = scope.run(() => exports.useChatSendQueue({
    busy: id => busy.has(id), valid: () => allowed,
    send: async item => { const done = deferred(); requests.push({ item, done }); await done.promise },
    interrupt: async item => { interrupts.push(item.id); await interrupt(item) },
  }))
  return { scope, queue, busy, requests, interrupts, stop: () => scope.stop(),
    invalid: () => allowed = false, onInterrupt: callback => interrupt = callback }
}

function editorHarness(h) {
  const input = vue.ref('original draft'), files = vue.ref([{ id: 'draft', name: 'draft.png', type: 'image' }])
  const currentScope = vue.ref('a')
  const editor = h.scope.run(() => exports.useChatQueueEditor({ queue: h.queue,
    scope: () => currentScope.value, input, files, focus: () => {} }))
  return { input, files, currentScope, editor }
}

test('main-composer editing holds FIFO and restores the draft while keeping the captured model', async () => {
  const h = harness(), e = editorHarness(h)
  h.busy.add('a')
  const row = { ...message('edit'), model: 'captured-model', files: [{id:'image',name:'queued.png',type:'image'}] }
  h.queue.enqueue(row); h.queue.enqueue(message('after'))
  e.editor.begin('edit')
  assert.equal(e.input.value, 'edit')
  e.files.value[0].detail = 'low'
  assert.equal(row.files[0].detail, undefined, 'editing attachment details must not mutate the queued payload')
  e.input.value = 'changed'; e.files.value = []
  h.busy.delete('a'); await turn()
  assert.equal(h.requests.length, 0, 'generation finishing cannot send the row being edited')
  assert.equal(e.editor.commit(), true); await turn()
  assert.equal(h.requests[0].item.id, 'edit')
  assert.equal(h.requests[0].item.model, 'captured-model')
  assert.equal(h.requests[0].item.text, 'changed')
  assert.deepEqual(h.requests[0].item.files, [])
  assert.equal(e.input.value, 'original draft')
  assert.equal(e.files.value[0].name, 'draft.png')
  assert.equal(e.editor.editingId.value, null)
  h.requests[0].done.resolve(); await turn()
  assert.equal(h.requests[1].item.id, 'after')
  h.requests[1].done.resolve(); h.stop()
})

test('cancelling, deleting a row and changing scope restore the draft without saving pending edits', async () => {
  for (const action of ['cancel', 'remove', 'scope', 'clear']) {
    const h = harness(), e = editorHarness(h)
    h.busy.add('a'); h.queue.enqueue(message('edit'))
    e.editor.begin('edit'); e.input.value = 'discard changes'; e.files.value = []
    if (action === 'cancel') e.editor.cancel()
    if (action === 'remove') h.queue.remove('edit')
    if (action === 'scope') e.currentScope.value = 'b'
    if (action === 'clear') h.queue.clear()
    assert.equal(e.input.value, 'original draft', action)
    assert.equal(e.files.value[0].name, 'draft.png', action)
    assert.equal(e.editor.editingId.value, null, action)
    assert.equal(h.requests.length, 0, action)
    if (h.queue.items.value.length) assert.equal(h.queue.items.value[0].text, 'edit')
    h.stop()
  }
})

test('switching edit targets holds the next row before releasing a finished first row', async () => {
  const h = harness(), e = editorHarness(h)
  h.busy.add('a'); h.queue.enqueue(message('first')); h.queue.enqueue(message('second'))
  e.editor.begin('first'); e.input.value = 'unsaved first changes'
  h.busy.delete('a'); await turn()
  e.editor.begin('second'); await turn()
  assert.equal(h.requests[0].item.text, 'first')
  assert.equal(e.input.value, 'second')
  h.requests[0].done.resolve(); await turn()
  assert.equal(h.requests.length, 1, 'the second row remains held')
  e.editor.cancel(); await turn()
  assert.equal(e.input.value, 'original draft')
  assert.equal(h.requests[1].item.text, 'second')
  h.requests[1].done.resolve(); h.stop()
})

test('empty edits remain held and cancelling after Stop does not resume a paused queue', async () => {
  const h = harness(), e = editorHarness(h)
  h.busy.add('a'); h.queue.enqueue(message('edit')); e.editor.begin('edit')
  e.input.value = ''; e.files.value = []
  assert.equal(e.editor.commit(), true)
  assert.equal(e.editor.editingId.value, 'edit')
  h.queue.pause('a'); h.busy.delete('a'); e.editor.cancel(); await turn()
  assert.equal(h.requests.length, 0)
  h.queue.resume('a'); await turn()
  assert.equal(h.requests[0].item.text, 'edit')
  h.requests[0].done.resolve(); h.stop()
})

test('FIFO waits for the full request, preserving captured attachments and payloads', async () => {
  const h = harness()
  h.queue.enqueue(message('one')); h.queue.enqueue(message('two')); h.queue.enqueue(message('three'))
  await turn()
  assert.deepEqual(h.requests.map(r => r.item.id), ['one'])
  h.requests[0].done.resolve(); await turn()
  assert.deepEqual(h.requests.map(r => r.item.id), ['one', 'two'])
  assert.equal(h.requests[1].item.files[0].name, 'paper.pdf')
  h.requests[1].done.resolve(); await turn()
  assert.equal(h.requests[2].item.id, 'three')
  h.requests[2].done.resolve(); h.stop()
})

test('regeneration or a mirrored stream blocks draining; different conversations run independently', async () => {
  const h = harness(); h.busy.add('a')
  h.queue.enqueue(message('wait')); h.queue.enqueue(message('other', 'b')); await turn()
  assert.deepEqual(h.requests.map(r => r.item.id), ['other'])
  h.busy.delete('a'); await turn()
  assert.equal(h.requests[1].item.id, 'wait')
  h.requests.forEach(r => r.done.resolve()); h.stop()
})

test('steering overtakes queued messages only after cancellation has settled', async () => {
  const h = harness(); const cancel = deferred()
  h.onInterrupt(async () => { await cancel.promise; h.requests[0].done.resolve() })
  h.queue.enqueue(message('first')); h.queue.enqueue(message('later')); await turn()
  h.queue.steer(message('urgent')); await turn()
  assert.deepEqual(h.interrupts, ['urgent'])
  assert.equal(h.requests.length, 1)
  cancel.resolve(); await turn()
  assert.equal(h.requests[1].item.id, 'urgent')
  h.requests[1].done.resolve(); await turn()
  assert.equal(h.requests[2].item.id, 'later')
  h.requests[2].done.resolve(); h.stop()
})

test('rapid steering does not double-submit; selecting an existing queue row sends it once', async () => {
  const h = harness(); h.busy.add('a'); const cancel = deferred()
  h.onInterrupt(async () => { await cancel.promise; h.busy.delete('a') })
  const row = message('row'); h.queue.enqueue(row)
  h.queue.steer(row); h.queue.steer(row); h.queue.steer(message('second')); await turn()
  assert.equal(h.interrupts.length, 1)
  cancel.resolve(); await turn()
  assert.equal(h.requests[0].item.id, 'row')
  h.requests[0].done.resolve(); await turn()
  assert.equal(h.requests[1].item.id, 'second')
  h.requests[1].done.resolve(); h.stop()
})

test('editing holds the first row when generation finishes; saved edits retain its position', async () => {
  const h = harness(); h.busy.add('a')
  h.queue.enqueue(message('edit')); h.queue.enqueue(message('after'))
  h.queue.hold('edit', true); h.busy.delete('a'); await turn()
  assert.equal(h.requests.length, 0)
  h.queue.edit('edit', 'changed', []); h.queue.hold('edit', false); await turn()
  assert.equal(h.requests[0].item.text, 'changed')
  assert.deepEqual(h.requests[0].item.files, [])
  h.queue.remove('after'); h.requests[0].done.resolve(); await turn()
  assert.equal(h.requests.length, 1); h.stop()
})

test('stop pauses the queue, resume drains, and deleting a scope cancels its remaining rows', async () => {
  const h = harness(); h.queue.enqueue(message('one')); h.queue.enqueue(message('two')); await turn()
  h.queue.pause('a'); h.requests[0].done.resolve(); await turn()
  assert.equal(h.requests.length, 1)
  h.queue.resume('a'); await turn()
  assert.equal(h.requests[1].item.id, 'two')
  h.queue.enqueue(message('deleted')); h.queue.clear('a')
  h.requests[1].done.resolve(); await turn()
  assert.equal(h.requests.length, 2); h.stop()
})

test('switching libraries or closing the component during cancellation cannot send an old queued message', async () => {
  for (const action of ['clear', 'dispose', 'invalid']) {
    const h = harness(); h.busy.add('a'); const cancel = deferred()
    h.onInterrupt(async () => { await cancel.promise; h.busy.delete('a') })
    h.queue.steer(message('old')); await turn()
    if (action === 'clear') h.queue.clear()
    if (action === 'dispose') h.stop()
    if (action === 'invalid') h.invalid()
    cancel.resolve(); await turn()
    assert.equal(h.requests.length, 0, action)
    h.stop()
  }
})
