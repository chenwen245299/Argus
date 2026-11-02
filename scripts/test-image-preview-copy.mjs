// Actual chat image previews and copy implementation, with clipboard IPC mocks.
// No real library, provider or system clipboard access.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')

const surfaces = {
  paper: 'src/components/tabs/AiTab.vue',
  library: 'src/components/LibraryChat.vue',
  canvas: 'src/components/tabs/CanvasChatTab.vue',
}
const harnesses = {}
for (const [surface, file] of Object.entries(surfaces)) {
  const source = await readFile(new URL('../' + file, import.meta.url), 'utf8')
  const overlay = source.match(/<div v-if="previewImage" class="(?:attachment-lightbox|cc-image-lightbox)"[\s\S]*?<\/div>/)?.[0]
  assert.ok(overlay, `image preview exists in ${file}`)
  const styles = [...source.matchAll(/\.(?:attachment-lightbox|lightbox-image|cc-image-lightbox)(?: img)?\s*\{[^}]*\}/g)].map(m => m[0]).join('\n')
  harnesses[new URL(`../__preview-${surface}.vue`, import.meta.url).pathname] = `<script setup>
    import{ref}from'vue';import{useI18n}from'vue-i18n';
    import ChatPageImage from'/src/components/ChatPageImage.vue';
    import ImagePreviewActions from'/src/components/ImagePreviewActions.vue';
    const{t}=useI18n(),previewImage=ref(null);
    function closePreview(){previewImage.value=null}
    const image=ref({slug:'paper',page:2,file:'figure.png',caption:'Figure 1',dataUrl:window.__test.src});
    window.__test.replace=value=>{previewImage.value=value};
  </script><template><div class="preview-host">
    <ChatPageImage conversation-id="mock-chat" :image="image" presentation="message" @open="previewImage=$event" />
    ${overlay}
  </div></template><style scoped>
    .preview-host{position:relative;width:360px;height:700px;max-width:100%} ${styles}
  </style>`
}
const mocks = {
  '@tauri-apps/api/core': `export const isTauri=()=>window.__test.native;export const invoke=async()=>window.__test.src;`,
  '@tauri-apps/api/image': `export class Image{static async new(rgba,width,height){window.__test.nativeImages.push({width,height,length:rgba.length,first:Array.from(rgba.slice(0,4)),last:Array.from(rgba.slice(-4))});return{close:async()=>{window.__test.closed++}}}}`,
  '@tauri-apps/plugin-clipboard-manager': `export const writeImage=async()=>{const t=window.__test;t.nativeWrites++;if(t.hold)await new Promise(resolve=>t.release=resolve);if(t.fail)throw new Error('Clipboard failure')};`,
}
let server, browser, page, origin
const errors = []
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'preview-clipboard-mocks', enforce: 'pre',
      resolveId: source => source in mocks ? '\0mock:' + source : /^\/__preview-(paper|library|canvas)\.vue$/.test(source) ? new URL('..' + source, import.meta.url).pathname : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : harnesses[id] ?? null,
    }, vue()], cacheDir: '/private/tmp/argus-preview-copy-vite',
    optimizeDeps: { entries: ['src/components/ImagePreviewActions.vue', 'src/components/ChatPageImage.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__test-preview')) return next()
    const surface = new URL(req.url, 'http://localhost').searchParams.get('surface') || 'paper'
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><div id="app"></div>
      <style>body{margin:0;padding:20px;box-sizing:border-box}#app{width:100%;height:100%}</style><script type="module">
      import{createApp}from'vue';import{createI18n}from'vue-i18n';import{zh,en}from'/src/i18n/locales.ts';
      import{addCollection}from'@iconify/vue';import fluent from'@iconify-json/fluent/icons.json';
      import'/src/assets/main.css';import'/src/assets/themes.css';addCollection(fluent);
      const canvas=document.createElement('canvas');canvas.width=1280;canvas.height=480;const ctx=canvas.getContext('2d');
      ctx.fillStyle='#fa1428';ctx.fillRect(0,0,640,480);ctx.fillStyle='#14c8f0';ctx.fillRect(640,0,640,480);
      ctx.fillStyle='white';ctx.font='48px sans-serif';ctx.fillText('Figure 1: original pixels',90,250);
      const t=window.__test={src:canvas.toDataURL(),native:true,nativeImages:[],nativeWrites:0,webWrites:[],closed:0,fail:false,hold:false};
      window.ClipboardItem=class{constructor(data){this.data=data}};
      Object.defineProperty(navigator,'clipboard',{value:{write:async items=>{
        const data=items[0].data['image/png'];const call={pending:data instanceof Promise,gesture:navigator.userActivation.isActive};t.webWrites.push(call);
        const blob=await data,bitmap=await createImageBitmap(blob);call.type=blob.type;call.width=bitmap.width;call.height=bitmap.height;bitmap.close();
      }}});
      const{default:Harness}=await import('/__preview-${surface}.vue');
      const i18n=createI18n({legacy:false,locale:'zh',messages:{zh,en}});t.locale=value=>i18n.global.locale.value=value;
      createApp(Harness).use(i18n).mount('#app');t.ready=true;
    </script>`))
  })
  await server.listen(); origin = 'http://127.0.0.1:' + server.httpServer.address().port
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 1000, height: 800 } })
  page.setDefaultTimeout(10000)
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await server?.close() })
async function open(surface = 'paper') {
  await page.goto(origin + '/__test-preview?surface=' + surface)
  await page.waitForFunction(() => window.__test?.ready)
  await page.locator('.chat-image-open').click()
  await page.getByRole('button', { name: '复制图片', exact: true }).waitFor()
}

for (const surface of Object.keys(surfaces)) {
  test(`${surface}: enlarged image has a copy button and copies full-resolution RGBA without closing`, async () => {
    await open(surface)
    const copy = page.getByRole('button', { name: '复制图片', exact: true })
    assert.equal(await page.evaluate(() => window.__test.nativeWrites), 0)
    const bounds = await copy.boundingBox()
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 1000)
    await copy.click()
    await page.getByRole('status').filter({ hasText: '图片已复制到剪贴板' }).waitFor()
    assert.deepEqual(await page.evaluate(() => ({ images: window.__test.nativeImages, writes: window.__test.nativeWrites, closed: window.__test.closed })), {
      images: [{ width: 1280, height: 480, length: 1280 * 480 * 4, first: [250,20,40,255], last: [20,200,240,255] }], writes: 1, closed: 1,
    })
    assert.equal(await page.locator('.image-preview-actions').count(), 1)
    if (process.env.ARGUS_TEST_OUTPUT_DIR) {
      await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
      await page.screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/copy-' + surface + '.png' })
    }
    await page.getByRole('button', { name: '关闭图片预览' }).click()
    assert.equal(await page.locator('.image-preview-actions').count(), 0)
  })
}

test('failed writes report failure, close native resources and allow retry; busy clicks do not duplicate writes', async () => {
  await open()
  await page.evaluate(() => { window.__test.fail = true })
  await page.getByRole('button', { name: '复制图片', exact: true }).click()
  await page.getByRole('status').filter({ hasText: '复制失败，请重试' }).waitFor()
  assert.equal(await page.evaluate(() => window.__test.closed), 1)
  await page.evaluate(() => { window.__test.fail = false; window.__test.hold = true })
  const button = page.getByRole('button', { name: '复制图片', exact: true })
  await button.click()
  await page.waitForFunction(() => window.__test.release)
  assert.equal(await button.isDisabled(), true)
  await button.evaluate(el => el.click())
  assert.equal(await page.evaluate(() => window.__test.nativeWrites), 2)
  await page.evaluate(() => window.__test.release())
  await page.getByRole('status').filter({ hasText: '图片已复制到剪贴板' }).waitFor()
  assert.equal(await page.evaluate(() => window.__test.closed), 2)
})

test('browser copying supplies a promised original-size PNG while the click gesture is active', async () => {
  await open('library')
  await page.evaluate(() => { window.__test.native = false; window.__test.locale('en') })
  await page.getByRole('button', { name: 'Copy image', exact: true }).click()
  await page.getByRole('status').filter({ hasText: 'Image copied to clipboard' }).waitFor()
  assert.deepEqual(await page.evaluate(() => window.__test.webWrites), [{ pending: true, gesture: true, type: 'image/png', width: 1280, height: 480 }])
  assert.equal(await page.evaluate(() => window.__test.nativeWrites), 0)
})

test('a new image clears stale feedback and narrow previews retain accessible copy/close buttons', async () => {
  await open()
  await page.getByRole('button', { name: '复制图片', exact: true }).click()
  await page.getByRole('status').waitFor()
  await page.evaluate(() => window.__test.replace(window.__test.src + '#different-image'))
  assert.equal(await page.getByRole('status').count(), 0)
  await page.setViewportSize({ width: 380, height: 800 })
  const bounds = await page.getByRole('button', { name: '复制图片', exact: true }).boundingBox()
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 380)
  await page.getByRole('button', { name: '复制图片', exact: true }).focus()
  await page.keyboard.press('Escape')
  assert.equal(await page.locator('.image-preview-actions').count(), 0)
  await page.setViewportSize({ width: 1000, height: 800 })
})

test('no browser errors', () => assert.deepEqual(errors, []))
