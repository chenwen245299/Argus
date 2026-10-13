// Run: node --test scripts/test-soclaas.mjs. Provider UI checks without desktop I/O.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

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
const { soclaasLogo, SOCLAAS_BASE_URL } = run(`${await moduleBody('../src/utils/soclaas.ts')}\nreturn { soclaasLogo, SOCLAAS_BASE_URL };`, { nusIcon: 'nus.png' })
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
