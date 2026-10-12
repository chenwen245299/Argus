// ARGUS_TEST_PLAYWRIGHT can point to a bundled Playwright index.mjs.
// New headless Chrome profile, localhost only; no real library or clipboard.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, mkdir } from 'node:fs/promises'
import { createServer } from 'node:http'
import { parse, compileScript, compileTemplate } from '@vue/compiler-sfc'
import ts from 'typescript'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const localeSource = await readFile(new URL('../src/i18n/locales.ts', import.meta.url), 'utf8')
const localeCode = ts.transpileModule(localeSource, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext } }).outputText
const { zh } = await import('data:text/javascript;base64,' + Buffer.from(localeCode).toString('base64'))
const source = await readFile(new URL('../src/components/WriteConfirmCard.vue', import.meta.url), 'utf8')
const { descriptor } = parse(source)
const compiled = compileScript(descriptor, { id: 'write-preview-test' })
const vueImports = (_, names) => `const {${names.replace(/\bas\b/g, ':')}} = window.Vue;`
const script = ts.transpileModule(compiled.content, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext } }).outputText
  .replace(/import \{([^}]+)\} from ['"]vue['"];?/g, vueImports)
  .replace(/import \{ Icon \} from ['"]@iconify\/vue['"];?/g, 'const Icon = { render: () => window.Vue.h("span") };')
  .replace(/import \{ useI18n \} from ['"]vue-i18n['"];?/g, String.raw`const useI18n = () => ({ t: (key, args = {}) => (window.translations[key.replace('writeConfirm.', '')] || key).replace(/\{(\w+)\}/g, (_, name) => args[name] ?? '') });`)
  .replace(/import MarkdownBody from ['"][^'"]+['"];?/g, `const MarkdownBody = { props: ['content'], render() { return window.Vue.h('div', this.content) } };`)
  .replace('export default', 'const component =')
const template = compileTemplate({ source: descriptor.template.content, filename: 'WriteConfirmCard.vue', id: 'write-preview-test',
  compilerOptions: { bindingMetadata: compiled.bindings } }).code
  .replace(/import \{([^}]+)\} from ['"]vue['"]/g, vueImports)
  .replace('export function render', 'function render')
const component = `(() => { ${script}\n${template}\ncomponent.render = render; window.Card = component; })();`
const styles = descriptor.styles[0].content.replace(/:deep\(([^)]+)\)/g, '$1')
const files = new Map([
  ['/vue.js', await readFile(new URL('../node_modules/vue/dist/vue.global.prod.js', import.meta.url))],
  ['/card.js', Buffer.from(component)],
  ['/', Buffer.from(`<!doctype html><meta charset="utf-8"><style>
    :root { --accent:#6386ae; --bg-primary:#f7f9fa; --bg-secondary:#edf1f3; --border-subtle:#e3e8ed;
      --text-primary:#202c3d; --text-secondary:#53677e; --text-tertiary:#8b9caf; --radius-md:8px; --radius-lg:12px; }
    body { padding:20px; font-family:Arial,sans-serif; } #app { max-width:620px; } ${styles}
    </style><div id="app"></div><script>window.translations = ${JSON.stringify(zh.writeConfirm)}</script><script src="/vue.js"></script><script src="/card.js"></script>`)],
])
let server, browser, page, origin
before(async () => {
  server = createServer((req, res) => {
    const body = files.get(req.url)
    res.setHeader('Content-Type', req.url.endsWith('.js') ? 'text/javascript' : 'text/html')
    res.writeHead(body ? 200 : 404); res.end(body)
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 700, height: 650 } })
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await new Promise(resolve => server ? server.close(resolve) : resolve()) })
async function mount(preview) {
  await page.goto(origin)
  await page.evaluate(preview => {
    window.decisions = []
    window.Vue.createApp({ render: () => window.Vue.h(window.Card, { preview, queued: 2,
      onApprove: () => window.decisions.push('approved'), onReject: () => window.decisions.push('rejected') }) }).mount('#app')
  }, preview)
}
test('a marking batch shows the exact copy, page indices, excerpts and annotations before approving', async () => {
  await mount({ tool: 'create_paper_annotations', slug: '2025_Energy-Based_Transformers-2', paperTitle: 'Energy-Based Transformers',
    noteTitle: '', content: '', duplicateTitle: false, passages: [
      { page: 4, text: 'To achieve all facets described, we propose viewing thinking as an optimization procedure.', note: '思考过程通过能量最小化逐步优化候选预测。', color: '#FFEB3B', style: 'highlight' },
      { page: 5, text: '<原文片段> System 2 Thinking with EBTs yields more substantial performance gains.', note: '这段强调泛化能力。', color: '#93C5FD', style: 'underline' },
    ] })
  assert.equal(await page.locator('.wc-passage').count(), 2)
  assert.match(await page.locator('.wc-location').innerText(), /Transformers-2/)
  assert.match(await page.locator('.wc-passage').nth(0).innerText(), /PDF 第 4 页/)
  assert.match(await page.locator('.wc-passage').nth(1).innerText(), /<原文片段>/)
  assert.equal(await page.locator('.wc-passage blockquote').nth(1).evaluate(el => el.children.length), 0, 'quotes are text, not executable HTML')
  assert.deepEqual(await page.evaluate(() => window.decisions), [])
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.locator('#app').screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/write-confirm.png` })
  }
  await page.locator('.wc-btn.primary').click()
  assert.deepEqual(await page.evaluate(() => window.decisions), ['approved'])
})
test('note previews and rejection still work with the existing payload shape', async () => {
  await mount({ tool: 'create_paper_note', slug: 'paper', paperTitle: 'A Paper', noteTitle: 'Reading notes', content: 'Existing note preview', duplicateTitle: true })
  assert.equal(await page.locator('.wc-passage').count(), 0)
  assert.match(await page.locator('.wc-fields').innerText(), /Reading notes/)
  assert.equal(await page.locator('.wc-warn').count(), 1)
  await page.locator('.wc-btn').first().click()
  assert.deepEqual(await page.evaluate(() => window.decisions), ['rejected'])
})
