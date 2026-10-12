// Run with Playwright installed: node --test scripts/test-mermaid.mjs.
// ARGUS_TEST_PLAYWRIGHT can point to a bundled Playwright index.mjs. Uses a new,
// headless Chrome profile and local scripts only; no library or clipboard access.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, mkdir } from 'node:fs/promises'
import { createServer } from 'node:http'
import { parse, compileScript, compileTemplate } from '@vue/compiler-sfc'
import ts from 'typescript'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const transpile = source => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const source = await readFile(new URL('../src/components/MermaidBlock.vue', import.meta.url), 'utf8')
const { descriptor } = parse(source)
const compiled = compileScript(descriptor, { id: 'mermaid-label-test' })
const script = transpile(compiled.content)
  .replace(/import \{([^}]+)\} from ['"]vue['"];?/g, (_, names) =>
    `const {${names.replace(/\bas\b/g, ':')}} = window.Vue;`)
  .replace(/import \{ Icon \} from ['"]@iconify\/vue['"];?/g, 'const Icon = { render: () => window.Vue.h("span") };')
  .replace(/import \{ useI18n \} from ['"]vue-i18n['"];?/g, 'const useI18n = () => ({ t: key => key });')
  .replace(/import DOMPurify from ['"]dompurify['"];?/g, 'const DOMPurify = window.DOMPurify;')
  .replace(/import \{ svgStringToPngBlob \} from ['"][^'"]+['"];?/g, 'const svgStringToPngBlob = window.svgStringToPngBlob;')
  .replace(/import \{ copyPngBlobToClipboard \} from ['"][^'"]+['"];?/g, 'const copyPngBlobToClipboard = async blob => { window.copiedDiagram = blob; };')
  .replace(/import\(['"]mermaid['"]\)/g, 'Promise.resolve({ default: window.mermaid })')
  .replace('export default', 'const component =')
const template = compileTemplate({ source: descriptor.template.content, filename: 'MermaidBlock.vue', id: 'mermaid-label-test',
  compilerOptions: { bindingMetadata: compiled.bindings } }).code
  .replace(/import \{([^}]+)\} from ['"]vue['"]/g, (_, names) =>
    `const {${names.replace(/\bas\b/g, ':')}} = window.Vue;`)
  .replace('export function render', 'function render')
const component = `(() => { ${script}\n${template}\ncomponent.render = render; window.MermaidBlock = component; })();`
const pngHelper = transpile(await readFile(new URL('../src/utils/svgToPng.ts', import.meta.url), 'utf8')).replace('export function', 'function')
const styles = descriptor.styles[0].content.replace(/:deep\(([^)]+)\)/g, '$1')
const files = new Map([
  ['/vue.js', await readFile(new URL('../node_modules/vue/dist/vue.global.prod.js', import.meta.url))],
  ['/mermaid.js', await readFile(new URL('../node_modules/mermaid/dist/mermaid.min.js', import.meta.url))],
  ['/purify.js', await readFile(new URL('../node_modules/dompurify/dist/purify.min.js', import.meta.url))],
  ['/component.js', Buffer.from(component)],
  ['/png.js', Buffer.from(pngHelper)],
  ['/', Buffer.from(`<!doctype html><meta charset="utf-8"><style>
    body { margin: 20px; font-family: Arial, sans-serif; --bg-secondary: #f4f6f8; }
    #app { width: 850px; } ${styles}
    </style><div id="app"></div><script src="/vue.js"></script><script src="/purify.js"></script>
    <script src="/mermaid.js"></script><script src="/png.js"></script><script src="/component.js"></script>`)],
])
let server, browser, page, origin
before(async () => {
  server = createServer((req, res) => {
    const body = files.get(req.url)
    if (!body) { res.writeHead(404); res.end(); return }
    res.setHeader('Content-Type', req.url.endsWith('.js') ? 'text/javascript' : 'text/html')
    res.end(body)
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 920, height: 1000 } })
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => {
  await browser?.close()
  await new Promise(resolve => server ? server.close(resolve) : resolve())
})

const ebt = `flowchart TD
  subgraph S["一次 thinking step（可重复 K 次，权重共享）"]
    direction TB
    zo["观察序列 z_o = Embed(x₁…x_N)<br/>B×N×D"]
    y["预测分布 ŷ<br/>B×N×V（V=50277）<br/>初始为随机噪声 N(0,I)"]
    proj["softmax → 线性投影到 embedding 空间<br/>(vocab_to_embed / Linear Projector)"]
    zp["预测表示 z_p<br/>B×N×D"]
    cat["拼接 [z_o ; z_p] → B×2N×D"]
    tr["Decoder-only Transformer × L<br/>Q/K/V 在 observed 与 predicted 间共享<br/>RMSNorm · SwiGLU · RoPE · 可学习 step embedding"]
    en["能量 E_θ(x, ŷ)<br/>B×N×1，只对预测位置输出"]
    y --> proj --> zp
    zo --> cat
    zp --> cat
    cat --> tr --> en
  end
  en -->|"∇_ŷ E"| upd["更新 ŷ ← ŷ − α ∇_ŷ E_θ(x, ŷ)<br/>S2 额外加朗之万噪声 η"]
  upd -->|"未到 K 步"| y
  upd -->|"最后一步"| loss["CrossEntropy(ŷ, x_next)<br/>只在最后一步算"]
  loss -.->|"反传穿过整个 K 步优化过程<br/>→ Hessian-vector product（二阶导）"| tr`

async function mount(src) {
  await page.goto(origin)
  await page.evaluate(src => { window.Vue.createApp(window.MermaidBlock, { src }).mount('#app'); }, src)
  await page.locator('.mermaid-inner svg, .mermaid-error').waitFor()
}

test('reproduces the original sanitizer removing every HTML label while retaining the boxes', async () => {
  await page.goto(origin)
  const result = await page.evaluate(async () => {
    mermaid.initialize({ startOnLoad: false, theme: 'neutral' })
    const { svg } = await mermaid.render('before-label-fix', 'flowchart LR\nA["上下文"] --> B["能量"]')
    const old = new DOMParser().parseFromString(svg, 'image/svg+xml')
    const clean = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } })
    const doc = new DOMParser().parseFromString(clean, 'image/svg+xml')
    return { hadHtml: old.querySelectorAll('foreignObject').length, remainingHtml: doc.querySelectorAll('foreignObject').length,
      labels: [...doc.querySelectorAll('text')].map(t => t.textContent).join(''), nodes: doc.querySelectorAll('g.node').length }
  })
  assert.ok(result.hadHtml > 0)
  assert.equal(result.remainingHtml, 0)
  assert.equal(result.labels, '')
  assert.equal(result.nodes, 2)
})

test('the real chat diagram retains Chinese, Greek, multiline and edge labels after sanitizing', async () => {
  await mount(ebt)
  const result = await page.locator('.mermaid-inner svg').evaluate(svg => ({
    labels: [...svg.querySelectorAll('text')].map(t => t.textContent).join('\n'),
    html: svg.querySelectorAll('foreignObject').length, nodes: svg.querySelectorAll('g.node').length,
  }))
  for (const label of ['观察序列', '预测分布 ŷ', 'vocab_to_embed', 'Q/K/V', '能量 E_θ', '∇_ŷ E', 'Hessian-vector product']) {
    // SVG wraps long labels into separate tspans without whitespace text nodes.
    assert.ok(result.labels.replace(/\s/g, '').includes(label.replace(/\s/g, '')), `retained label: ${label}`)
  }
  assert.equal(result.nodes, 9)
  assert.equal(result.html, 0)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.locator('#app').screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/ebt.png` })
  }
})

test('diagram directives cannot restore the HTML labels that would be stripped', async () => {
  for (const prefix of [
    "%%{init: {'htmlLabels': true, 'flowchart': {'htmlLabels': true}}}%%\n",
    '---\nconfig:\n  htmlLabels: true\n---\n',
  ]) {
    await mount(prefix + 'flowchart LR\nA["中文<br/>第二行"] --> B["结果"]')
    const result = await page.locator('.mermaid-inner svg').evaluate(svg => ({
      html: svg.querySelectorAll('foreignObject').length,
      labels: [...svg.querySelectorAll('text')].map(t=>t.textContent).join(''),
    }))
    assert.equal(result.html, 0)
    assert.ok(result.labels.includes('中文') && result.labels.includes('第二行') && result.labels.includes('结果'))
  }
})

test('copy-as-image produces PNG text pixels, not only boxes and arrows', async () => {
  await mount('flowchart LR\nA["上下文 x"] --> B["能量 E(x,y)"]')
  await page.locator('.mermaid-copy-btn').click({ force: true })
  await page.waitForFunction(() => !!window.copiedDiagram)
  const result = await page.evaluate(async () => {
    const image = await createImageBitmap(window.copiedDiagram)
    const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height
    const ctx = canvas.getContext('2d'); ctx.drawImage(image, 0, 0)
    const svg = document.querySelector('.mermaid-inner svg'), label = svg.querySelector('g.node text')
    const matrix = svg.getCTM().inverse().multiply(label.getCTM()), box = label.getBBox(), vb = svg.viewBox.baseVal
    const points = [[box.x,box.y],[box.x+box.width,box.y+box.height]].map(([x,y])=>new DOMPoint(x,y).matrixTransform(matrix))
    const x = Math.max(0,Math.floor((points[0].x-vb.x)*image.width/vb.width)), y = Math.max(0,Math.floor((points[0].y-vb.y)*image.height/vb.height))
    const w = Math.min(image.width-x,Math.ceil((points[1].x-points[0].x)*image.width/vb.width))
    const h = Math.min(image.height-y,Math.ceil((points[1].y-points[0].y)*image.height/vb.height))
    const pixels = ctx.getImageData(x,y,w,h).data
    let dark = 0
    for (let i=0;i<pixels.length;i+=4) if (pixels[i+3]>128 && pixels[i]<100 && pixels[i+1]<100 && pixels[i+2]<100) dark++
    return { type:window.copiedDiagram.type,width:image.width,height:image.height,dark }
  })
  assert.equal(result.type, 'image/png')
  assert.ok(result.width>0 && result.height>0 && result.dark>10)
})

test('invalid Mermaid source still falls back to readable code', async () => {
  const src = 'flowchart LR\nA["unfinished'
  await mount(src)
  assert.equal(await page.locator('.mermaid-error').textContent(), src)
  assert.equal(await page.locator('.mermaid-inner svg').count(), 0)
})
