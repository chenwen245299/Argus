// Run: node --test scripts/test-paper-ai-window.mjs. Local window/AI mocks only.
import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parse, compileScript } from '@vue/compiler-sfc'
import ts from 'typescript'

const dir = await mkdtemp(join(tmpdir(), 'argus-paper-ai-test-'))
after(() => rm(dir, { recursive: true, force: true }))
const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const helper = await readFile(new URL('../src/utils/paperAiWindow.ts', import.meta.url), 'utf8')
await writeFile(join(dir, 'paperAiWindow.mjs'), transpile(helper))
const { paperAiWindowSlug } = await import(pathToFileURL(join(dir, 'paperAiWindow.mjs')))

// Execute the real view's setup and lifecycle with delayed provider loading.
// The old shared localStorage handoff made both views load the latest paper.
await writeFile(join(dir, 'mocks.mjs'), `
export { ref, watch } from '${import.meta.resolve('vue')}'
export const defineComponent = component => component
export const mounted = []
export const onMounted = callback => mounted.push(callback)
export const onUnmounted = () => {}
let label
export const selectWindow = next => { label = next }
export const getCurrentWebviewWindow = () => ({ label, setTitle: async () => {} })
export const getCurrentWindow = () => ({ onResized: async () => () => {} })
let finishLoading
const loading = new Promise(resolve => { finishLoading = resolve })
const ai = { loaded: false, load: () => loading }
export const useAiStore = () => ai
export const finishLoad = () => { ai.loaded = true; finishLoading() }
export const invoke = async (command, args) => {
  if (command === 'get_paper_meta') return { title: args.slug }
  throw new Error('Unexpected command: ' + command)
}
export const listen = async () => () => {}
export default {}
`)
const source = await readFile(new URL('../src/views/PaperAiView.vue', import.meta.url), 'utf8')
const compiled = transpile(compileScript(parse(source).descriptor, { id: 'paper-ai-test' }).content)
await writeFile(join(dir, 'PaperAiView.mjs'), compiled
  .replace(/from '(?:vue|@tauri-apps\/api\/[^']+|\.\.\/stores\/ai|\.\.\/components\/tabs\/AiTab.vue)'/g, "from './mocks.mjs'")
  .replace(/from '\.\.\/utils\/paperAiWindow'/g, "from './paperAiWindow.mjs'"))
const { default: view } = await import(pathToFileURL(join(dir, 'PaperAiView.mjs')))
const mocks = await import(pathToFileURL(join(dir, 'mocks.mjs')))

test('decodes backend labels without losing Unicode, spaces or punctuation', () => {
  // The Chinese fixture is also asserted by the Rust label tests.
  assert.equal(paperAiWindowSlug('paper-ai-6K665paHIEE'), '论文 A')
  for (const slug of ['paper-a', 'paper a', 'paper_a', '研究 🧪', 'a'.repeat(255)]) {
    assert.equal(paperAiWindowSlug(`paper-ai-${Buffer.from(slug).toString('base64url')}`), slug)
  }
  for (const label of ['main', 'paper-ai', 'paper-ai-', 'paper-ai-!', 'paper-ai-a', 'paper-ai-_w']) {
    assert.equal(paperAiWindowSlug(label), null)
  }
})

test('two paper windows and a reload retain their own paper through delayed startup', async () => {
  const previousStorage = globalThis.localStorage
  const previousDocument = globalThis.document
  globalThis.localStorage = { getItem: () => 'paper-b', setItem: () => {} }
  globalThis.document = { title: '' }
  try {
    mocks.selectWindow('paper-ai-cGFwZXItYQ')
    const first = view.setup({}, { expose() {} })
    const firstMount = mocks.mounted.at(-1)()
    mocks.selectWindow('paper-ai-cGFwZXItYg')
    const second = view.setup({}, { expose() {} })
    const secondMount = mocks.mounted.at(-1)()
    mocks.finishLoad()
    await Promise.all([firstMount, secondMount])
    assert.equal(first.slug.value, 'paper-a')
    assert.equal(second.slug.value, 'paper-b')

    // Reload A while the legacy shared handoff still points at B.
    mocks.selectWindow('paper-ai-cGFwZXItYQ')
    const reopened = view.setup({}, { expose() {} })
    await mocks.mounted.at(-1)()
    assert.equal(reopened.slug.value, 'paper-a')
    assert.equal(second.slug.value, 'paper-b')
  } finally {
    globalThis.localStorage = previousStorage
    globalThis.document = previousDocument
  }
})
