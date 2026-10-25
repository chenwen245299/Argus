// The real PdfViewer, with App.vue's global selection rules, on a generated two-page PDF whose
// citation links point at hyperref-style `cite.` anchors. Desktop calls are mocked; no library
// or provider is used. Run with ARGUS_TEST_PLAYWRIGHT pointing to a Playwright index.mjs (or with
// playwright installed) and Chrome present. ARGUS_TEST_OUTPUT_DIR keeps screenshots.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')

// ── Fixture ──────────────────────────────────────────────────────────────────
// Page 1 cites (Smith et al., 2020) and Figure 1; page 2 is a reference list with hanging indents,
// margin line numbers and anchors placed the way natbib + hyperref place them (25 pt left of the
// entry, raised about one line).
const REFERENCE = 'Carol Smith, Dan Brown, and Eve White. Self-consistency in large language '
  + 'models. arXiv preprint arXiv:2001.00001, 2020.'

function fixturePdf() {
  const esc = s => s.replace(/[\\()]/g, m => '\\' + m)
  const text = (x, y, s, size = 10) => `BT /F1 ${size} Tf ${x} ${y} Td (${esc(s)}) Tj ET`
  const page1 = [
    text(72, 700, 'Self-consistency selects an answer by voting'),
    text(262, 700, '(Smith et al., 2020).'),
    text(72, 680, 'Its behaviour is shown in'),
    text(184, 680, 'Figure 1'),
    text(222, 680, 'of this short test document.'),
    text(72, 500, 'Figure 1: A caption about 2020 results, after Jones et al.'),
  ].join('\n')
  const entries = [
    [715, 72, 'Alice Jones and Bob Lee. A first paper about things. In Proceedings of the'],
    [703, 82, 'Conference on Examples, pages 1-10, 2019.'],
    [685, 72, 'Carol Smith, Dan Brown, and Eve White. Self-consistency in large language'],
    [673, 82, 'models. arXiv preprint arXiv:2001.00001, 2020.'],
    [655, 72, 'Fay Zhou. Another reference entry. Journal of Tests, 3(1):1-2, 2021.'],
  ]
  const page2 = [
    text(72, 740, 'References', 12),
    ...entries.map(([y, x, s], i) => `${text(40, y, String(101 + i), 7)}\n${text(x, y, s)}`),
  ].join('\n')
  const anchor = baseline => `/XYZ 47 ${baseline + 11.5} null`
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R /Names << /Dests 3 0 R >> >>',
    '<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>',
    `<< /Names [(cite.jones2019) [5 0 R ${anchor(715)}] (cite.smith2020) [5 0 R ${anchor(685)}] `
      + `(cite.zhou2021) [5 0 R ${anchor(655)}] (figure.1) [4 0 R /XYZ 72 520 null]] >>`,
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R /Annots [8 0 R 9 0 R] >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 6 0 R >> >> /Contents 10 0 R >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>',
    `<< /Length ${page1.length} >>\nstream\n${page1}\nendstream`,
    '<< /Type /Annot /Subtype /Link /Rect [260 696 342 710] /Border [0 0 0] /Dest (cite.smith2020) >>',
    '<< /Type /Annot /Subtype /Link /Rect [183 676 219 690] /Border [0 0 0] /Dest (figure.1) >>',
    `<< /Length ${page2.length} >>\nstream\n${page2}\nendstream`,
  ]
  let pdf = '%PDF-1.7\n'
  const offsets = []
  objects.forEach((body, i) => { offsets.push(pdf.length); pdf += `${i + 1} 0 obj\n${body}\nendobj\n` })
  const xref = pdf.length
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`
  pdf += offsets.map(o => `${String(o).padStart(10, '0')} 00000 n \n`).join('')
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
  return Buffer.from(pdf, 'latin1')
}

// ── Harness ──────────────────────────────────────────────────────────────────
let server, browser, context, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args); export const convertFileSrc=p=>p; export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async()=>()=>{}; export const emit=async()=>{}; export const emitTo=async()=>{};`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'main',onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}});export class Window{static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'main'});export class WebviewWindow{static async getByLabel(){return null}}`,
}

before(async () => {
  const pdf = fixturePdf()
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-citation-preview', enforce: 'pre', resolveId: s => s in mocks ? '\0mock:' + s : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null }, vue()],
    optimizeDeps: { entries: ['src/components/PdfViewer.vue'] },
    // Its own dependency cache: browser tests started side by side (node --test runs files in
    // parallel) would otherwise re-optimize each other's shared cache and break the page load.
    cacheDir: 'node_modules/.vite-citation-preview-test',
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (req.url === '/__citation/fixture.pdf') {
      res.setHeader('Content-Type', 'application/pdf')
      return res.end(pdf)
    }
    if (!req.url.startsWith('/__citation')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
    <style>html,body,#app{margin:0;width:100%;height:100%}</style><script type="module">
    import {createApp,h} from 'vue';import {createPinia} from 'pinia';import {createI18n} from 'vue-i18n';
    import {addCollection} from '@iconify/vue';import fluent from '@iconify-json/fluent/icons.json';
    import PdfViewer from '/src/components/PdfViewer.vue';
    import {useReaderStore} from '/src/stores/reader.ts';import {useLibraryStore} from '/src/stores/library.ts';
    import {zh,en} from '/src/i18n/locales.ts';
    import 'pdfjs-dist/web/pdf_viewer.css';import '/src/assets/main.css';import '/src/assets/themes.css';
    import '/src/App.vue?vue&type=style&index=0&lang.css';
    import {installMarkdownCopy} from '/src/utils/copyAsMarkdown.ts';
    addCollection(fluent);installMarkdownCopy();
    window.__test={calls:[],async invoke(name){this.calls.push(name);
      if(name==='read_pdf_bytes')return Array.from(new Uint8Array(await (await fetch('/__citation/fixture.pdf')).arrayBuffer()));
      if(name==='get_highlights')return [];return null}};
    const pinia=createPinia();
    const app=createApp({render:()=>h('div',{class:'app-root',style:'height:100vh;display:flex;flex-direction:column'},[h(PdfViewer,{slug:'paper'})])});
    app.use(pinia).use(createI18n({legacy:false,locale:'zh',fallbackLocale:'en',messages:{zh,en}}));
    useLibraryStore(pinia).papers=[{slug:'paper',title:'Citation test'}];
    useReaderStore(pinia).openPaper('paper','Citation test');
    app.mount('#app');
    </script>`))
  })
  await server.listen()
  origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  context = await browser.newContext({ viewport: { width: 1100, height: 900 } })
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin })
  page = await context.newPage()
  page.on('pageerror', e => errors.push(String(e)))
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()) })
  await page.goto(`${origin}/__citation`)
  await page.waitForSelector('[data-page-index="0"] .annotationLayer a[data-element-id="8R"]', { timeout: 60000 })
})

after(async () => {
  await browser?.close()
  await server?.close()
})

const smith = '[data-page-index="0"] .annotationLayer a[data-element-id="8R"]'
const figure = '[data-page-index="0"] .annotationLayer a[data-element-id="9R"]'
const popup = '.ref-popup'

async function centre(selector) {
  const box = await page.locator(selector).boundingBox()
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 }
}

async function hover(selector) {
  const { x, y } = await centre(selector)
  await page.mouse.move(x, y, { steps: 3 })
  return Date.now()
}

async function reset() {
  await page.keyboard.press('Escape')
  await page.mouse.click(900, 860)
  await page.waitForSelector(popup, { state: 'detached' })
}

async function shot(name) {
  if (!process.env.ARGUS_TEST_OUTPUT_DIR) return
  await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
  await page.screenshot({ path: join(process.env.ARGUS_TEST_OUTPUT_DIR, `citation-preview-${name}.png`) })
}

test('hovering a citation for half a second shows its full reference, without the native tooltip', async () => {
  const started = await hover(smith)
  await page.waitForTimeout(300)
  assert.equal(await page.locator(popup).count(), 0, 'not before the delay')
  await page.waitForSelector(popup, { timeout: 5000 })
  assert.ok(Date.now() - started >= 450, 'shown after about 0.5 s')
  assert.equal((await page.locator(`${popup} .ref-popup-text`).textContent()).trim(), REFERENCE)
  assert.equal(await page.locator(smith).getAttribute('title'), null)
  await shot('open')
  await reset()
})

test('the pointer can travel onto the preview, select its text and copy it', async t => {
  t.after(reset)
  await hover(smith)
  await page.waitForSelector(popup)
  const link = await page.locator(smith).boundingBox()
  const box = await page.locator(`${popup} .ref-popup-text`).boundingBox()
  // Across the gap between the link and the preview, more slowly than the grace period.
  await page.mouse.move(link.x + 4, link.y + link.height + 2, { steps: 2 })
  await page.mouse.move(box.x + 6, box.y + 6, { steps: 6 })
  await page.waitForTimeout(600)
  assert.equal(await page.locator(popup).count(), 1, 'still open over the preview')

  // A drag selection inside it, released inside: the preview stays, and the PDF's own
  // selection toolbar does not open for it.
  await page.mouse.down()
  await page.mouse.move(box.x + box.width - 6, box.y + box.height - 6, { steps: 8 })
  await page.mouse.up()
  const selected = await page.evaluate(() => window.getSelection().toString().replace(/\s+/g, ' ').trim())
  assert.ok(selected.length > 20 && REFERENCE.includes(selected.slice(0, 20)), `selected: ${selected}`)
  await page.waitForTimeout(400)
  assert.equal(await page.locator(popup).count(), 1, 'still open over the preview after selecting')
  assert.equal(await page.locator('.sel-popup').count(), 0)

  // Copying the selection (what ⌘C does) gives exactly the selected text: neither the viewer's
  // own copy handler nor the app's markdown copy takes it over.
  await page.evaluate(() => document.execCommand('copy'))
  const copied = await page.evaluate(() => navigator.clipboard.readText())
  assert.equal(copied.replace(/\s+/g, ' ').trim(), selected)

  await page.locator(`${popup} .ref-popup-copy`).click()
  await page.waitForSelector(`${popup} .ref-popup-copy.done`)
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), REFERENCE)

  // Copy releases the pin left by the selection: moving away closes it again.
  await page.mouse.move(900, 860)
  await page.waitForSelector(popup, { state: 'detached', timeout: 2000 })
})

test('a finished selection does not keep the preview: moving away closes it', async t => {
  t.after(reset)
  await hover(smith)
  await page.waitForSelector(popup)
  const box = await page.locator(`${popup} .ref-popup-text`).boundingBox()
  await page.mouse.move(box.x + 6, box.y + 6, { steps: 6 })
  await page.mouse.down()
  await page.mouse.move(box.x + box.width - 6, box.y + box.height - 6, { steps: 8 })
  await page.mouse.up()
  await page.waitForTimeout(400)
  assert.equal(await page.locator(popup).count(), 1)
  // No copy: just leave.
  await page.mouse.move(900, 860, { steps: 4 })
  await page.waitForSelector(popup, { state: 'detached', timeout: 2000 })
})

test('a selection drag may run past the preview while held; released outside, it closes', async t => {
  t.after(reset)
  await hover(smith)
  await page.waitForSelector(popup)
  const box = await page.locator(`${popup} .ref-popup-text`).boundingBox()
  await page.mouse.move(box.x + 6, box.y + 6, { steps: 6 })
  await page.mouse.down()
  await page.mouse.move(box.x + box.width - 6, box.y + box.height - 6, { steps: 8 })
  await page.mouse.move(box.x + box.width + 120, box.y + box.height + 60, { steps: 4 })
  await page.waitForTimeout(500)
  assert.equal(await page.locator(popup).count(), 1, 'kept open while the button is held')
  // The page's text stays out of the selection.
  const selected = await page.evaluate(() => window.getSelection().toString().replace(/\s+/g, ' ').trim())
  assert.ok(selected.length > 20 && REFERENCE.includes(selected), `selected: ${selected}`)
  await page.mouse.up()
  await page.waitForSelector(popup, { state: 'detached', timeout: 2000 })
  assert.equal(await page.locator('.sel-popup').count(), 0)
})

test('leaving the link before the delay, or hovering a figure link, shows nothing', async t => {
  t.after(reset)
  await hover(smith)
  await page.waitForTimeout(200)
  await page.mouse.move(900, 860, { steps: 2 })
  await page.waitForTimeout(900)
  assert.equal(await page.locator(popup).count(), 0)

  await hover(figure)
  await page.waitForTimeout(1200)
  assert.equal(await page.locator(popup).count(), 0)
  await page.mouse.move(900, 860)
})

test('Escape closes it, and it stays inside a narrow window', async t => {
  t.after(async () => { await reset(); await page.setViewportSize({ width: 1100, height: 900 }) })
  await hover(smith)
  await page.waitForSelector(popup)
  await page.keyboard.press('Escape')
  await page.waitForSelector(popup, { state: 'detached' })

  await page.setViewportSize({ width: 480, height: 700 })
  await page.mouse.move(470, 690)
  await page.locator(smith).scrollIntoViewIfNeeded()
  await page.waitForTimeout(300)
  await hover(smith)
  await page.waitForSelector(popup)
  const box = await page.locator(popup).boundingBox()
  assert.ok(box.x >= 8 && box.x + box.width <= 480 - 8 + 0.5, `inside the window: ${JSON.stringify(box)}`)
  await shot('narrow')
  await reset()
  await page.setViewportSize({ width: 1100, height: 900 })
})

test('no page errors', () => {
  assert.deepEqual(errors.filter(e => !/favicon|standard font|Warning/i.test(e)), [])
})
