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
const { icons } = JSON.parse(await readFile(new URL('../node_modules/@iconify-json/fluent/icons.json', import.meta.url), 'utf8'))
const previewIcons = Object.fromEntries(['copy', 'arrow-expand', 'subtract', 'add', 'scale-fit', 'dismiss']
  .map(name => [`${name}-24-regular`, icons[`${name}-24-regular`]]))
const locales = transpile(await readFile(new URL('../src/i18n/locales.ts', import.meta.url), 'utf8'))
  .replace(/export const /g, 'const ')
const { descriptor } = parse(source)
const compiled = compileScript(descriptor, { id: 'mermaid-label-test' })
const script = transpile(compiled.content)
  .replace(/import \{([^}]+)\} from ['"]vue['"];?/g, (_, names) =>
    `const {${names.replace(/\bas\b/g, ':')}} = window.Vue;`)
  .replace(/import \{ Icon \} from ['"]@iconify\/vue['"];?/g, `const previewIcons = ${JSON.stringify(previewIcons)};
    const Icon = { props: ['icon'], render() { return window.Vue.h('svg', { viewBox: '0 0 24 24', innerHTML: previewIcons[this.icon.replace('fluent:', '')]?.body || '' }); } };`)
  .replace(/import \{ useI18n \} from ['"]vue-i18n['"];?/g, 'const useI18n = () => ({ t: key => key.split(".").reduce((value, part) => value?.[part], window.zh) || key });')
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
  ['/locales.js', Buffer.from(`${locales}\nwindow.zh = zh;`)],
  ['/main.css', await readFile(new URL('../src/assets/main.css', import.meta.url))],
  ['/themes.css', await readFile(new URL('../src/assets/themes.css', import.meta.url))],
  ['/', Buffer.from(`<!doctype html><meta charset="utf-8"><style>
    ${styles}
    #app { width: 850px; height: calc(100% - 40px); margin: 20px; overflow: auto; }
    </style><link rel="stylesheet" href="/main.css"><link rel="stylesheet" href="/themes.css">
    <div id="app"></div><script src="/vue.js"></script><script src="/purify.js"></script><script src="/locales.js"></script>
    <script src="/mermaid.js"></script><script src="/png.js"></script><script src="/component.js"></script>`)],
])
let server, browser, page, origin
const errors = []
before(async () => {
  server = createServer((req, res) => {
    const body = files.get(req.url)
    if (!body) { res.writeHead(404); res.end(); return }
    res.setHeader('Content-Type', req.url.endsWith('.js') ? 'text/javascript' : req.url.endsWith('.css') ? 'text/css' : 'text/html')
    res.end(body)
  })
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true, timeout: 30_000 })
  page = await browser.newPage({ viewport: { width: 920, height: 1000 } })
  page.setDefaultTimeout(10_000)
  page.on('pageerror', error => errors.push(error.message))
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
  await page.evaluate(src => { window.app = window.Vue.createApp(window.MermaidBlock, { src }); window.app.mount('#app'); }, src)
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
  assert.equal(await page.getByRole('dialog').count(), 0, 'copy does not also open the diagram')
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

const wide = `flowchart LR
  subgraph SMILES["SMILES Stream"]
    A["L 条随机 DeepSMILES 路径"] --> B["Token Emb + Token Gaussian Basis"]
  end
  subgraph Fusion["Fusion Layers × 8（d=512, 8 heads）"]
    C["各流内 Self-Attention（AdaLN 注入 c 和 t）"] --> D["SMILES 专有：Inter-Path Attention"] --> E["流间 Cross-Attention"] --> F["SwiGLU FFN"]
  end
  B --> C
  G["谱峰 m/z, intensity, top-200"] --> H["Gaussian Basis 编码 + Spec 相对 m/z 位置偏置"] --> C
  I["ECFP 向量 → FP Compress"] --> C
  F --> J["SMILES Head → 每个位置的 token 分布"]
  E --> K["Spectrum Head → 速度场 v_t"]
  E --> L["FP Head → 指纹向量"]`

async function waitForPreview() {
  await page.getByRole('dialog').waitFor()
  await page.waitForFunction(() => {
    const image = document.querySelector('.mermaid-preview-stage img')
    return image?.complete && image.naturalWidth > 0
  })
}
const previewWidth = () => page.locator('.mermaid-preview-stage img').evaluate(image => image.getBoundingClientRect().width)

test('hover reveals enlarge and copy; the enlarged vector diagram fits the window', async () => {
  await mount(wide)
  await page.locator('#app').evaluate(el => { el.style.width = '340px' })
  await page.mouse.move(0, 0)
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.mermaid-actions')).opacity === '0')
  await page.locator('.mermaid-inner').hover()
  await page.waitForFunction(() => getComputedStyle(document.querySelector('.mermaid-actions')).opacity === '1')
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.locator('.mermaid-block').screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/mermaid-hover.png` })
  }
  const inlineWidth = await page.locator('.mermaid-inner svg').evaluate(svg => svg.getBoundingClientRect().width)
  await page.getByRole('button', { name: '放大查看', exact: true }).last().click()
  await waitForPreview()
  assert.ok(await previewWidth() > inlineWidth * 2, 'the narrow chat diagram grows to use the window')
  const fit = await page.locator('.mermaid-preview-viewport').evaluate(el => ({
    width: el.clientWidth, height: el.clientHeight, scrollWidth: el.scrollWidth, scrollHeight: el.scrollHeight,
  }))
  assert.ok(fit.scrollWidth <= fit.width + 1 && fit.scrollHeight <= fit.height + 1, 'fit shows the whole diagram')
  assert.equal(await page.locator('.mermaid-preview-stage img').evaluate(image => {
    const svg = new DOMParser().parseFromString(decodeURIComponent(image.src.split(',')[1]), 'image/svg+xml')
    return svg.querySelectorAll('foreignObject, script, parsererror').length
  }), 0)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/mermaid-preview.png` })
  }
})

test('clicking the diagram supports zoom buttons, wheel zoom, panning and fit reset', async () => {
  await mount(wide)
  await page.locator('.mermaid-inner svg').click()
  await waitForPreview()
  const fittedWidth = await previewWidth()
  await page.getByRole('button', { name: '放大', exact: true }).click()
  assert.ok(await previewWidth() > fittedWidth * 1.2)
  await page.getByRole('button', { name: '缩小', exact: true }).click()
  assert.ok(Math.abs(await previewWidth() - fittedWidth) < 1)
  const viewport = page.locator('.mermaid-preview-viewport')
  const bounds = await viewport.boundingBox()
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
  await page.mouse.wheel(0, -500)
  await page.waitForFunction(width => document.querySelector('.mermaid-preview-stage img').getBoundingClientRect().width > width, fittedWidth)
  for (let i = 0; i < 5; i++) await page.getByRole('button', { name: '放大', exact: true }).click()
  const before = await viewport.evaluate(el => el.scrollLeft)
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
  await page.mouse.down()
  await page.mouse.move(bounds.x + bounds.width / 2 - 100, bounds.y + bounds.height / 2 - 60, { steps: 5 })
  await page.mouse.up()
  assert.ok(await viewport.evaluate(el => el.scrollLeft) > before + 90, 'drag pans the enlarged diagram')
  await page.getByRole('button', { name: '适应窗口' }).click()
  assert.ok(Math.abs(await previewWidth() - fittedWidth) < 1)
  assert.equal(await viewport.evaluate(el => el.scrollLeft + el.scrollTop), 0)
  await page.getByRole('button', { name: '关闭图表预览' }).click()
  assert.equal(await page.getByRole('dialog').count(), 0)
})

test('keyboard entry, focus loop, Escape, backdrop and unmount clean up the preview', async () => {
  await mount(wide)
  const opener = page.locator('.mermaid-inner')
  await opener.focus()
  await page.keyboard.press('Enter')
  await waitForPreview()
  assert.equal(await page.getByRole('button', { name: '关闭图表预览' }).evaluate(el => el === document.activeElement), true)
  await page.keyboard.press('Tab')
  assert.equal(await page.getByRole('button', { name: '缩小' }).evaluate(el => el === document.activeElement), true)
  await page.keyboard.press('Shift+Tab')
  assert.equal(await page.getByRole('button', { name: '关闭图表预览' }).evaluate(el => el === document.activeElement), true)
  await page.keyboard.press('Escape')
  assert.equal(await page.getByRole('dialog').count(), 0)
  assert.equal(await opener.evaluate(el => el === document.activeElement), true)
  await page.keyboard.press('Space')
  await waitForPreview()
  await page.locator('.mermaid-preview-overlay').click({ position: { x: 3, y: 3 } })
  assert.equal(await page.getByRole('dialog').count(), 0)
  await opener.click()
  await waitForPreview()
  await page.evaluate(() => window.app.unmount())
  assert.equal(await page.locator('.mermaid-preview-overlay').count(), 0)
})

test('the preview remains usable in a narrow dark window', async () => {
  await page.setViewportSize({ width: 390, height: 700 })
  try {
    await mount(wide)
    await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; document.querySelector('#app').style.width = '340px' })
    await page.locator('.mermaid-inner').click()
    await waitForPreview()
    const bounds = await page.getByRole('dialog').boundingBox()
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 390)
    assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= 700)
    assert.equal(await page.locator('.mermaid-preview-viewport').evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(255, 255, 255)')
    if (process.env.ARGUS_TEST_OUTPUT_DIR) {
      await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/mermaid-preview-dark-narrow.png` })
    }
  } finally {
    await page.setViewportSize({ width: 920, height: 1000 })
  }
})

test('invalid Mermaid source still falls back to readable code', async () => {
  const src = 'flowchart LR\nA["unfinished'
  await mount(src)
  assert.equal(await page.locator('.mermaid-error').textContent(), src)
  assert.equal(await page.locator('.mermaid-inner svg').count(), 0)
})

test('the component produces no browser errors', () => assert.deepEqual(errors, []))
