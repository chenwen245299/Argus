// Real image component with deferred desktop reads; no library or provider access.
// ARGUS_TEST_PLAYWRIGHT may point to a bundled Playwright index.mjs.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { parse, compileScript, compileTemplate } from '@vue/compiler-sfc'
import ts from 'typescript'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const { descriptor } = parse(await readFile(new URL('../src/components/ChatPageImage.vue', import.meta.url), 'utf8'))
const compiled = compileScript(descriptor, { id: 'chat-page-image-test' })
const globals = code => code.replace(/import \{([^}]+)\} from ['"]vue['"];?/g, (_, names) =>
  `const {${names.replace(/\bas\b/g, ':')}} = window.Vue;`)
const script = globals(ts.transpileModule(compiled.content, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText)
  .replace(/import \{ invoke \} from ['"]@tauri-apps\/api\/core['"];?/g, 'const invoke = (...args) => window.__test.invoke(...args);')
  .replace(/import \{ Icon \} from ['"]@iconify\/vue['"];?/g, 'const Icon = { render: () => window.Vue.h("span") };')
  .replace(/import \{ useI18n \} from ['"]vue-i18n['"];?/g, 'const useI18n = () => ({ t: key => key });')
  .replace('export default', 'const component =')
const template = globals(compileTemplate({ source: descriptor.template.content, filename: 'ChatPageImage.vue',
  id: 'chat-page-image-test', compilerOptions: { bindingMetadata: compiled.bindings } }).code)
  .replace('export function render', 'function render')
const files = new Map([
  ['/vue.js', await readFile(new URL('../node_modules/vue/dist/vue.global.prod.js', import.meta.url))],
  ['/component.js', Buffer.from(`(() => { ${script}\n${template}\ncomponent.render = render; window.ChatPageImage = component; })();`)],
  ['/', Buffer.from(`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
    <style>${descriptor.styles[0].content}</style><script src="/vue.js"></script><script src="/component.js"></script>`)],
])
let server, browser, page, origin
const errors = []
before(async () => {
  server = createServer((req, res) => {
    const body = files.get(req.url)
    if (!body) { res.writeHead(404); res.end(); return }
    res.setHeader('Content-Type', req.url.endsWith('.js') ? 'text/javascript' : 'text/html')
    res.end(body)
  })
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', resolve)
  })
  origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true, timeout: 30_000 })
  page = await browser.newPage()
  page.setDefaultTimeout(5_000)
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => {
  await browser?.close()
  await new Promise(resolve => server ? server.close(resolve) : resolve())
})

// A local PNG is enough to exercise the real <img> loading and DOM lifetime.
const png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6ZucAAAAASUVORK5CYII='
async function mount(image, presentation = 'thumbnail') {
  await page.goto(origin)
  await page.evaluate(({ image, presentation }) => {
    const { ref, h, createApp } = window.Vue
    const current = ref(image), conversation = ref('chat-a')
    window.__test = {
      image: current, conversation, reads: [], opened: [],
      invoke(command, args) {
        if (command !== 'read_chat_image') throw new Error(command)
        return new Promise((resolve, reject) => this.reads.push({ args, resolve, reject }))
      },
    }
    createApp({ render: () => h(window.ChatPageImage, {
      image: current.value, conversationId: conversation.value, presentation,
      onOpen: src => window.__test.opened.push(src),
    }) }).mount('#app')
  }, { image, presentation })
}
async function resolveRead(index = 0, src = png) {
  await page.evaluate(async ({ index, src }) => {
    window.__test.reads[index].resolve(src)
    await Promise.resolve(); await window.Vue.nextTick()
  }, { index, src })
  await page.locator('img').waitFor()
  await page.waitForFunction(() => document.querySelector('img')?.complete && document.querySelector('img')?.naturalWidth > 0)
}
const reads = () => page.evaluate(() => window.__test.reads.map(read => read.args))

for (const presentation of ['thumbnail', 'message']) {
  test(`${presentation}: cloned sync snapshots retain the loaded image and perform one disk read`, async () => {
    await mount({ slug: 'paper', page: 21, file: 'page-a.png' }, presentation)
    await resolveRead()
    const state = await page.evaluate(async () => {
      const img = document.querySelector('img')
      for (let i = 0; i < 10; i++) {
        window.__test.image.value = { ...window.__test.image.value, caption: `snapshot ${i}` }
        await window.Vue.nextTick()
        if (document.querySelector('img') !== img) return { retained: false, reads: window.__test.reads.length }
      }
      return { retained: true, reads: window.__test.reads.length }
    })
    assert.deepEqual(state, { retained: true, reads: 1 })
    await page.locator('button').click()
    assert.deepEqual(await page.evaluate(() => window.__test.opened), [png])
  })

  test(`${presentation}: persisting a live image keeps its pixels when the data URL is dropped`, async () => {
    await mount({ slug: 'paper', page: 21, file: 'page-a.png', dataUrl: png }, presentation)
    const retained = await page.evaluate(async () => {
      const img = document.querySelector('img')
      const { dataUrl, ...saved } = window.__test.image.value
      window.__test.image.value = saved
      await window.Vue.nextTick()
      return document.querySelector('img') === img
    })
    assert.equal(retained, true)
    assert.deepEqual(await reads(), [])
  })
}

test('snapshots arriving during an outstanding read do not restart or invalidate it', async () => {
  await mount({ slug: 'paper', page: 21, file: 'page-a.png' })
  await page.evaluate(async () => {
    for (let i = 0; i < 5; i++) {
      window.__test.image.value = { ...window.__test.image.value }
      await window.Vue.nextTick()
    }
  })
  assert.equal((await reads()).length, 1)
  await resolveRead()
})

test('changing the file or conversation clears old pixels and ignores stale reads', async () => {
  await mount({ slug: 'paper', page: 21, file: 'page-a.png' })
  await page.evaluate(async () => {
    window.__test.image.value = { slug: 'paper', page: 22, file: 'page-b.png' }
    await window.Vue.nextTick()
  })
  assert.deepEqual(await reads(), [
    { conversationId: 'chat-a', file: 'page-a.png' },
    { conversationId: 'chat-a', file: 'page-b.png' },
  ])
  await page.evaluate(async src => {
    window.__test.reads[0].resolve(src)
    await Promise.resolve(); await window.Vue.nextTick()
  }, png)
  assert.equal(await page.locator('img').count(), 0)
  await resolveRead(1)
  await page.evaluate(async () => {
    window.__test.conversation.value = 'chat-b'
    await window.Vue.nextTick()
  })
  assert.equal(await page.locator('img').count(), 0)
  assert.deepEqual((await reads()).at(-1), { conversationId: 'chat-b', file: 'page-b.png' })
  await resolveRead(2)
})

test('a live result supersedes an outstanding disk read', async () => {
  await mount({ slug: 'paper', page: 21, file: 'page-a.png' })
  await page.evaluate(async src => {
    window.__test.image.value = { ...window.__test.image.value, dataUrl: src }
    await window.Vue.nextTick()
    window.__test.reads[0].reject(new Error('stale disk failure'))
    await Promise.resolve(); await window.Vue.nextTick()
  }, png)
  assert.equal(await page.locator('img').getAttribute('src'), png)
  assert.equal((await reads()).length, 1)
})

test('an unavailable saved image reports failure without retrying on every snapshot', async () => {
  await mount({ slug: 'paper', page: 21, file: 'missing.png' }, 'message')
  await page.evaluate(async () => {
    window.__test.reads[0].reject(new Error('missing file'))
    await Promise.resolve(); await window.Vue.nextTick()
    window.__test.image.value = { ...window.__test.image.value }
    await window.Vue.nextTick()
  })
  assert.equal((await reads()).length, 1)
  assert.equal(await page.locator('.chat-image-placeholder').innerText(), 'chat.imageUnavailable')
})

test('the component produces no browser errors', () => assert.deepEqual(errors, []))
