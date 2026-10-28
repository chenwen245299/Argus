// Run: node --test scripts/test-soclaas.mjs. Provider UI checks without desktop I/O.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
import { createPinia, defineStore } from 'pinia'
import { ref, watch, onScopeDispose } from 'vue'

async function moduleBody(path) {
  const source = await readFile(new URL(path, import.meta.url), 'utf8')
  const ast = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true)
  return ast.statements.filter(n => !ts.isImportDeclaration(n)).map(n => n.getText(ast))
    .join('\n').replace(/^export /gm, '').replaceAll('import.meta.glob', 'assetGlob')
}
function run(source, env) {
  return new Function(...Object.keys(env), ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
  }).outputText)(...Object.values(env))
}
const { soclaasLogo, SOCLAAS_BASE_URL, isSoclaas } = run(`${await moduleBody('../src/utils/soclaas.ts')}\nreturn { soclaasLogo, SOCLAAS_BASE_URL, isSoclaas };`, { nusIcon: 'nus.png' })
const { soclaasReasoningEfforts, resolveReasoningEffort, standardReasoningLevel } = run(
  `${await moduleBody('../src/utils/reasoning.ts')}\nreturn { soclaasReasoningEfforts, resolveReasoningEffort, standardReasoningLevel };`, { isSoclaas })
const assetGlob = () => ({
  '/src/assets/providers/openai.svg': { default: 'openai.svg' },
  '/src/assets/providers/deepseek.svg': { default: 'deepseek.svg' },
})
const providerLogo = run(`${await moduleBody('../src/utils/providerLogo.ts')}\nreturn providerLogo;`, { soclaasLogo, assetGlob })
const modelLogo = run(`${await moduleBody('../src/utils/modelLogo.ts')}\nreturn modelLogo;`, { soclaasLogo, assetGlob })
const model = (providerName, modelId = 'x-test-1') => ({ providerName, modelId, providerId: 'custom-id', displayName: modelId })

test('NUS branding applies to current generic configurations and the dedicated type', () => {
  assert.equal(SOCLAAS_BASE_URL, 'https://soclaas-api.comp.nus.edu.sg/v1')
  assert.equal(providerLogo('SoCLaaS', 'https://relay.example/v1'), 'nus.png')
  assert.equal(providerLogo('Research', SOCLAAS_BASE_URL), 'nus.png')
  assert.equal(providerLogo('Research', 'https://relay.example/v1', 'soclaas'), 'nus.png')
  assert.equal(modelLogo(model('SoCLaaS')), 'nus.png')
  assert.equal(modelLogo(model('Research'), 'openai_compatible', SOCLAAS_BASE_URL), 'nus.png')
  assert.equal(modelLogo(model('Research', 'gpt-private-alias'), 'soclaas'), 'nus.png')
})

test('existing provider and model branding stays intact', () => {
  assert.equal(providerLogo('OpenAI', 'https://api.openai.com/v1'), 'openai.svg')
  assert.equal(providerLogo('DeepSeek'), 'deepseek.svg')
  assert.equal(modelLogo(model('OpenAI', 'gpt-test'), 'openai_compatible'), 'openai.svg')
  assert.equal(modelLogo(model('DeepSeek', 'deepseek-test')), 'deepseek.svg')
})

test('the SoCLaaS preset uses the documented public gateway', async () => {
  const source = await readFile(new URL('../src/components/settings/AiSettings.vue', import.meta.url), 'utf8')
  const ast = ts.createSourceFile('AiSettings.ts', source.slice(source.indexOf('>') + 1, source.indexOf('</script>')), ts.ScriptTarget.Latest, true)
  const declaration = ast.statements.find(n => ts.isVariableStatement(n) && n.declarationList.declarations.some(d => d.name.getText(ast) === 'PRESETS'))
  const presets = run(`${declaration.getText(ast)}\nreturn PRESETS;`, { SOCLAAS_BASE_URL })
  const preset = presets.find(p => p.kind === 'soclaas')
  assert.equal(preset.base_url, SOCLAAS_BASE_URL)
  assert.match(preset.label, /NUS/)
})

test('discovered options use actual values, survive alias changes, and leave other providers alone', () => {
  const provider = { name: 'Research', kind: 'soclaas', models: [{ id: 'alias', reasoning_efforts: ['max', 'high', 'low', 'none'] }] }
  assert.deepEqual(soclaasReasoningEfforts(provider, 'alias'), ['none', 'low', 'high', 'max'])
  provider.models[0].reasoning_efforts = ['xhigh', 'medium', 'low', 'low']
  assert.deepEqual(soclaasReasoningEfforts(provider, 'alias'), ['low', 'medium', 'xhigh'])
  assert.deepEqual(soclaasReasoningEfforts(provider, 'new-model'), [])
  assert.equal(soclaasReasoningEfforts({ ...provider, kind: 'openai_compatible' }, 'alias'), null)
})

test('visible effort resolves to the same valid selection as the backend', () => {
  for (const [supported, preferred, enabled, expected] of [
    [['max', 'high', 'low'], 'max', true, 'max'],
    [['max', 'high', 'low'], 'medium', true, 'high'],
    [['max', 'high', 'low'], 'xhigh', true, 'max'],
    [['xhigh', 'medium', 'low'], 'high', true, 'xhigh'],
    [['max', 'xhigh', 'high', 'medium', 'low'], 'xhigh', true, 'xhigh'],
    [['max', 'high', 'low'], 'max', false, 'low'],
    [['max', 'high', 'low', 'none'], 'max', false, 'none'],
    [[], 'high', true, undefined],
    [['none'], 'high', true, undefined],
  ]) assert.equal(resolveReasoningEffort(supported, enabled, preferred), expected)
  assert.equal(standardReasoningLevel('max'), 'high')
  assert.equal(standardReasoningLevel('xhigh'), 'high')
  assert.equal(standardReasoningLevel('minimal'), 'low')
  assert.equal(standardReasoningLevel('medium'), 'medium')
})

test('thinking preferences persist provider-declared efforts and follow other windows', async t => {
  const stored = new Map([['argus:thinking-preference', JSON.stringify({ enabled: true, level: 'max' })]])
  const callbacks = new Map()
  const storage = { getItem: k => stored.get(k) ?? null, setItem: (k, v) => stored.set(k, v) }
  const window = { addEventListener: (k, cb) => callbacks.set(k, cb), removeEventListener: k => callbacks.delete(k) }
  const useStore = run(`${await moduleBody('../src/stores/thinking.ts')}\nreturn useThinkingStore;`,
    { ref, watch, onScopeDispose, defineStore, window, localStorage: storage })
  const store = useStore(createPinia())
  t.after(() => store.$dispose())
  assert.equal(store.reasoningLevel, 'max')
  store.reasoningLevel = 'xhigh'
  assert.equal(JSON.parse(stored.get('argus:thinking-preference')).level, 'xhigh')
  callbacks.get('storage')({ key: 'argus:thinking-preference', newValue: JSON.stringify({ enabled: false, level: 'medium' }) })
  assert.equal(store.useReasoning, false)
  assert.equal(store.reasoningLevel, 'medium')
  // Malformed storage never becomes an arbitrary request parameter.
  callbacks.get('storage')({ key: 'argus:thinking-preference', newValue: JSON.stringify({ level: 'not an effort' }) })
  assert.equal(store.reasoningLevel, 'high')
})
