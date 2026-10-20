// Real Pinia stores in separate window scopes, with local desktop/storage mocks.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { defineStore, createPinia } from 'pinia'
import { ref, computed, onScopeDispose } from 'vue'
import ts from 'typescript'

const source = await readFile(new URL('../src/stores/ai.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('ai.ts', source, ts.ScriptTarget.Latest, true)
const code = ts.transpileModule(ast.statements.filter(s => !ts.isImportDeclaration(s)).map(s => s.getText(ast)).join('\n'), {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText.replace(/^export /gm, '')
const makeStore = new Function('defineStore', 'ref', 'computed', 'onScopeDispose', 'invoke', 'listen', `${code}\nreturn useAiStore`)
const clone = value => JSON.parse(JSON.stringify(value))
const model = id => ({ id, display_name: id, capabilities: ['tool_calling'] })
const initial = () => ({ default_provider_id: 'p', default_model_id: 'original', providers: [{
  id: 'p', name: 'Example', kind: 'custom', base_url: 'https://example.test/v1', enabled: true,
  models: [model('original')],
}] })
const tick = () => new Promise(resolve => setTimeout(resolve, 0))
async function until(check) {
  for (let i = 0; i < 100; i++) { if (check()) return; await tick() }
  assert.fail('Expected store update did not arrive')
}
function fixture() {
  const b = { root: '/library-a', settings: new Map([['/library-a', initial()], ['/library-b', initial()]]),
    listeners: new Map(), stores: [], reads: [], held: [], hold: false, failSave: false, calls: [],
    balanceHeld: [], holdBalances: false, delayedListeners: [], delayListeners: false }
  b.emit = (name, payload) => { for (const cb of b.listeners.get(name) || []) cb({ payload }) }
  b.window = id => {
    const listen = async (name, callback) => {
      if (b.delayListeners) await new Promise(resolve => b.delayedListeners.push(resolve))
      const set = b.listeners.get(name) || new Set(); b.listeners.set(name, set); set.add(callback)
      return () => set.delete(callback)
    }
    const invoke = async (name, args) => {
      b.calls.push({ id, name, args })
      if (name === 'get_current_library') return b.root
      if (name === 'get_ai_settings') {
        assert.equal(args.expectedRoot, b.root)
        const value = clone(b.settings.get(b.root)); b.reads.push({ id, root: b.root })
        if (b.hold) return new Promise(resolve => b.held.push({ id, value, resolve }))
        return value
      }
      if (name === 'fetch_provider_balances') {
        const value = [{ providerId: 'p', balance: { marker: b.root } }]
        if (b.holdBalances) return new Promise(resolve => b.balanceHeld.push({ value, resolve }))
        return value
      }
      if (b.failSave) throw new Error('Save failed')
      const settings = b.settings.get(b.root), p = settings.providers.find(p => p.id === args.id)
      if (name === 'save_provider_models') p.models = clone(args.models)
      else if (name === 'set_provider_enabled') p.enabled = args.enabled
      else if (name === 'set_default_model') {
        settings.default_provider_id = args.providerId; settings.default_model_id = args.modelId
      } else if (name === 'update_ai_provider') Object.assign(settings.providers.find(p => p.id === args.provider.id), args.provider)
      else if (name === 'delete_ai_provider') settings.providers = settings.providers.filter(p => p.id !== args.id)
      else throw new Error(`Unexpected command ${name}`)
      b.emit('ai-settings-changed', { root: b.root })
    }
    const useStore = makeStore(defineStore, ref, computed, onScopeDispose, invoke, listen)
    const store = useStore(createPinia()); b.stores.push(store); return store
  }
  b.dispose = () => b.stores.forEach(store => store.$dispose())
  return b
}

test('saving new models refreshes every already-open window without recreating stores', async t => {
  const b = fixture(); t.after(b.dispose)
  const settings = b.window('settings'), paper = b.window('paper'), library = b.window('library')
  await Promise.all([settings.load(), paper.load(), library.load()])
  await settings.saveModels('p', [model('original'), model('new-model')])
  await until(() => paper.chatModels.length === 2 && library.chatModels.length === 2)
  assert.equal(paper.groupedModels[0].models[1].modelId, 'new-model')
  assert.equal(paper.findModel({ providerId: 'p', modelId: 'original' }).modelId, 'original')
})

test('provider edits, disabling, defaults and deletion propagate to other windows', async t => {
  const b = fixture(); t.after(b.dispose)
  const settings = b.window('settings'), paper = b.window('paper')
  await Promise.all([settings.load(), paper.load()])
  await settings.updateProvider({ ...clone(settings.settings.providers[0]), name: 'Renamed' })
  await until(() => paper.chatModels[0].providerName === 'Renamed')
  await settings.saveModels('p', [model('original'), model('second')])
  await settings.setDefault({ providerId: 'p', modelId: 'second' })
  await until(() => paper.defaultSelection?.modelId === 'second')
  await settings.setProviderEnabled('p', false)
  await until(() => !paper.isConfigured)
  await settings.setProviderEnabled('p', true)
  await until(() => paper.chatModels.length === 2)
  await settings.deleteProvider('p')
  await until(() => paper.settings.providers.length === 0)
})

test('a failed save does not publish unsaved models to any window', async t => {
  const b = fixture(); t.after(b.dispose)
  const settings = b.window('settings'), paper = b.window('paper')
  await Promise.all([settings.load(), paper.load()]); const reads = b.reads.length
  b.failSave = true
  await assert.rejects(settings.saveModels('p', [model('unsaved')]), /Save failed/)
  await tick()
  assert.equal(b.reads.length, reads)
  assert.deepEqual(paper.chatModels.map(m => m.modelId), ['original'])
})

test('a slow earlier load cannot replace a newly saved model list', async t => {
  const b = fixture(); t.after(b.dispose)
  const paper = b.window('paper'); await paper.load()
  b.hold = true; const oldLoad = paper.load(); await until(() => b.held.length === 1)
  b.hold = false; b.settings.get(b.root).providers[0].models.push(model('newest'))
  b.emit('ai-settings-changed', { root: b.root })
  await until(() => paper.chatModels.length === 2)
  b.held[0].resolve(b.held[0].value); await oldLoad
  assert.deepEqual(paper.chatModels.map(m => m.modelId), ['original', 'newest'])
})

test('library changes discard old models, balances and notifications', async t => {
  const b = fixture(); t.after(b.dispose)
  const paper = b.window('paper'); b.holdBalances = true; await paper.load()
  await until(() => b.balanceHeld.length === 1)
  b.hold = true; const stale = paper.load(); await until(() => b.held.length === 1)
  b.root = '/library-b'; b.settings.get(b.root).providers[0].models = [model('library-b-model')]
  b.hold = false; b.holdBalances = false; b.emit('library-changed', b.root)
  await until(() => paper.chatModels[0]?.modelId === 'library-b-model' && paper.balances.p?.marker === '/library-b')
  b.held[0].resolve(b.held[0].value); b.balanceHeld[0].resolve(b.balanceHeld[0].value); await stale; await tick()
  assert.equal(paper.chatModels[0].modelId, 'library-b-model')
  assert.equal(paper.balances.p.marker, '/library-b')
  const reads = b.reads.length
  b.emit('ai-settings-changed', { root: '/library-a' }); await tick()
  assert.equal(b.reads.length, reads)
})

test('listener registration precedes the first read and disposal removes late listeners', async t => {
  const b = fixture(); t.after(b.dispose)
  b.delayListeners = true; const paper = b.window('paper'); const loading = paper.load()
  await tick(); assert.equal(b.reads.length, 0)
  paper.$dispose(); b.delayedListeners.forEach(resolve => resolve()); await loading
  assert.equal(b.reads.length, 0)
  assert.equal([...b.listeners.values()].reduce((n, set) => n + set.size, 0), 0)
})
