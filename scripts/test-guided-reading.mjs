// Local browser regression: real Vue panels and intro, with desktop I/O mocked.
// ARGUS_TEST_PLAYWRIGHT may point to a bundled Playwright index.mjs.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, context, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (command, args) => window.__test.invoke(command, args);
    export const convertFileSrc = path => path; export const isTauri = () => false;`,
  '@tauri-apps/api/event': `export const listen = async (name, callback) => {
    const callbacks = window.__test.listeners.get(name) || new Set();
    window.__test.listeners.set(name, callbacks); callbacks.add(callback);
    return () => callbacks.delete(callback);
  }; export const emit = async (name, payload) => window.__test.emit(name, payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow = () => ({ label: 'main',
    onResized: async () => () => {}, onFocusChanged: async () => () => {} });`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow = () => ({ label: 'main' });
    export class WebviewWindow { static async getByLabel() { return null; } }`,
}

before(async () => {
  server = await createServer({
    configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-desktop', enforce: 'pre',
      resolveId: source => source in mocks ? '\0mock:' + source : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null,
    }, vue()],
    optimizeDeps: { entries: ['src/components/tabs/AiTab.vue', 'src/components/GuidedReadingIntro.vue'] },
    server: { host: '127.0.0.1', port: 0, strictPort: false }, logLevel: 'error',
  })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__guided-test')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    const html = `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html, body, #app { margin: 0; width: 100%; height: 100%; } .test-panel { height: 100vh; }</style>
      <script type="module">
      import { createApp, h, ref, nextTick } from 'vue';
      import { createPinia } from 'pinia'; import { createI18n } from 'vue-i18n';
      import { addCollection } from '@iconify/vue'; import fluent from '@iconify-json/fluent/icons.json';
      import AiTab from '/src/components/tabs/AiTab.vue';
      import Intro from '/src/components/GuidedReadingIntro.vue';
      import { useAiStore } from '/src/stores/ai.ts';
      import { useSettingsStore } from '/src/stores/settings.ts';
      import { zh, en } from '/src/i18n/locales.ts';
      import * as guided from '/src/utils/guidedReading.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      addCollection(fluent);
      const query = new URLSearchParams(location.search), mode = query.get('mode') || 'intro';
      const test = window.__test = { requests: [], saved: [], listeners: new Map(), guided,
        emit(name, payload) { for (const cb of test.listeners.get(name) || []) cb({ payload }); },
        invoke(command, args) {
          if (command === 'get_current_library') return Promise.resolve('/mock-library');
          if (command === 'get_paper_meta') return Promise.resolve({ title: 'Example paper', abstract: 'A complete paper' });
          if (command === 'get_paper_ai_conversations' || command === 'get_paper_chat') return Promise.resolve([]);
          if (command === 'save_paper_ai_conversations') test.saved.push(structuredClone(args));
          if (command === 'chat_with_library') return new Promise((resolve, reject) => test.requests.push({ args, resolve, reject }));
          if (command === 'cancel_ai_request') test.cancelled = args.requestId;
          return Promise.resolve(null);
        },
      };
      const introReady = ref(query.get('ready') !== 'false'), introKey = ref(0), showIntro = ref(mode === 'intro');
      test.ready = value => introReady.value = value;
      test.reopenIntro = async () => { showIntro.value = false; await nextTick(); introKey.value++; showIntro.value = true; };
      test.showIntro = () => showIntro.value = true;
      const app = createApp({ setup() { return () => h('div', { class: 'test-panel' }, [
        mode !== 'intro' ? h(AiTab, { slug: 'paper', standalone: mode === 'standalone' }) : null,
        showIntro.value ? h(Intro, { ready: introReady.value, key: introKey.value }) : null,
      ]); } });
      const pinia = createPinia(); app.use(pinia);
      app.use(createI18n({ legacy: false, locale: query.get('locale') || 'zh', messages: { zh, en } }));
      const ai = useAiStore(pinia); ai.loaded = true;
      ai.settings = { default_provider_id: 'provider', default_model_id: 'test-vision', providers: [{
        id: 'provider', name: 'Example', enabled: true, kind: 'custom', base_url: 'https://example.test/v1',
        models: [{ id: 'test-vision', display_name: 'Test vision', capabilities: ['vision', 'tool_calling'] }],
      }] };
      useSettingsStore(pinia).load = async () => {};
      app.mount('#app'); test.mounted = true;
      </script>`
    res.end(await server.transformIndexHtml(req.url, html))
  })
  await server.listen()
  origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  context = await browser.newContext({ viewport: { width: 900, height: 780 } })
  await context.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
  page = await context.newPage()
  page.on('pageerror', error => { errors.push(error.message); console.error('Browser error:', error.message) })
})
after(async () => { await browser?.close(); await server?.close() })

async function open(mode = 'intro', extra = '') {
  await page.goto(`${origin}/__guided-test?mode=${mode}${extra}`)
  await page.waitForFunction(() => window.__test?.mounted)
  await page.evaluate(() => localStorage.clear())
  if (mode === 'intro') await page.evaluate(() => window.__test.reopenIntro())
  else await page.locator('.guided-reading-btn').waitFor({ state: 'visible' })
}
async function start() {
  await page.locator('.guided-reading-btn').click()
  await page.waitForFunction(() => window.__test.requests.length > 0)
}
async function finish(text = '先想象一个走迷宫的机器人。它需要选择下一步问什么。这个场景清楚吗？') {
  await page.evaluate(text => window.__test.requests.at(-1).resolve(text), text)
  await page.locator('.stop-btn').waitFor({ state: 'hidden' })
}
async function flags() {
  return page.evaluate(() => ({
    experienced: localStorage.getItem(window.__test.guided.GUIDED_READING_EXPERIENCED_KEY),
    dismissed: localStorage.getItem(window.__test.guided.GUIDED_READING_DISMISSED_KEY),
  }))
}

test('both default prompts request full paginated text, visual inspection, concise teaching and a pause for understanding', async () => {
  await open()
  const prompts = await page.evaluate(() => ['zh', 'en'].map(locale => window.__test.guided.guidedReadingPrompt(locale)))
  for (const prompt of prompts) {
    assert.match(prompt, /get_paper_fulltext/)
    assert.match(prompt, /has_more=false/)
    assert.match(prompt, /view_paper_page/)
    assert.match(prompt, /send_paper_image/)
    assert.match(prompt, /附录|appendices/)
    assert.match(prompt, /图表|figures/)
    assert.match(prompt, /等我|Wait for my/)
    assert.match(prompt, /3—6|3–6/)
    assert.match(prompt, /例子|example/)
  }
})

test('intro waits for setup, appears once per launch, and transient acknowledgement does not suppress the next launch', async () => {
  await open('intro', '&ready=false')
  assert.equal(await page.getByRole('dialog').count(), 0)
  await page.evaluate(() => window.__test.ready(true))
  await page.getByRole('dialog').waitFor()
  assert.ok(await page.getByRole('button', { name: '我知道了' }).evaluate(el => document.activeElement === el))
  await page.getByRole('button', { name: '我知道了' }).click()
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await page.evaluate(() => { window.__test.ready(false); window.__test.ready(true) })
  assert.equal(await page.getByRole('dialog').count(), 0)
  await page.reload()
  await page.waitForFunction(() => window.__test?.mounted)
  await page.evaluate(() => window.__test.ready(true))
  await page.getByRole('dialog').waitFor()
})

test('the checkbox persists only with acknowledgement; Escape and the close button never save it', async () => {
  await open()
  await page.getByRole('checkbox').check()
  await page.keyboard.press('Escape')
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await page.evaluate(() => window.__test.reopenIntro())
  await page.getByRole('checkbox').check()
  await page.getByRole('button', { name: '关闭', exact: true }).click()
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await page.evaluate(() => window.__test.reopenIntro())
  await page.getByRole('checkbox').check()
  await page.getByRole('button', { name: '我知道了' }).click()
  assert.deepEqual(await flags(), { experienced: null, dismissed: '1' })
  await page.reload()
  await page.waitForFunction(() => window.__test?.mounted)
  assert.equal(await page.getByRole('dialog').count(), 0)
})

test('sidebar button immediately sends the preset, retains the draft, prevents duplicate clicks, and follow-ups stay in the same conversation', async () => {
  await open('sidebar')
  const textarea = page.locator('textarea').first()
  await textarea.fill('我的未发送草稿')
  // Queue a real attachment through the actual file input; the preset must not send it.
  await page.locator('input[type=file]').setInputFiles({ name: 'draft.png', mimeType: 'image/png',
    buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==', 'base64') })
  await page.getByText('draft.png', { exact: true }).waitFor()
  await start()
  assert.equal(await textarea.inputValue(), '我的未发送草稿')
  assert.ok(await page.locator('.guided-reading-btn').isDisabled())
  const first = await page.evaluate(() => window.__test.requests[0].args)
  assert.equal(first.paperSlug, 'paper')
  assert.equal(first.knowledgeSource, 'agent')
  assert.equal(first.modelId, 'test-vision')
  assert.equal(first.messages.at(-1).content, await page.evaluate(() => window.__test.guided.guidedReadingPrompt('zh')))
  assert.equal(await page.locator('.user-bubble').first().innerText(), '请通读这篇论文和所有图表，用易懂的例子带我逐轮读懂。')
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await finish()
  assert.deepEqual(await flags(), { experienced: '1', dismissed: null })
  assert.ok(await page.getByText('draft.png', { exact: true }).isVisible())
  assert.equal(await page.locator('.header-title').innerText(), 'AI 领读')
  const saved = await page.evaluate(() => window.__test.saved.at(-1).conversations)
  assert.ok(saved.some(conv => conv.source === 'guidedReading'))

  await textarea.fill('继续')
  await page.locator('.send-btn').click()
  await page.waitForFunction(() => window.__test.requests.length === 2)
  const next = await page.evaluate(() => window.__test.requests[1].args)
  assert.equal(next.conversationId, first.conversationId)
  assert.equal(next.messages[0].content, first.messages[0].content)
  assert.ok(next.messages.some(m => m.role === 'assistant' && m.content.includes('走迷宫')))
  assert.ok(next.messages.at(-1).content.some(part => part.type === 'text' && part.text.includes('继续')))
  await finish('下一点：机器人要比较每个问题能带来多少信息。')
  assert.equal(await page.locator('.header-title').innerText(), 'AI 领读')
})

test('standalone entry uses the same request and failures or empty replies do not count as experiencing the feature', async () => {
  await open('standalone')
  await start()
  await page.evaluate(() => window.__test.requests[0].reject(new Error('API 400')))
  await page.locator('.stop-btn').waitFor({ state: 'hidden' })
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await page.locator('.guided-reading-btn').click()
  await page.waitForFunction(() => window.__test.requests.length === 2)
  await finish('')
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  await page.locator('.guided-reading-btn').click()
  await page.waitForFunction(() => window.__test.requests.length === 3)
  await finish()
  assert.deepEqual(await flags(), { experienced: '1', dismissed: null })
})

test('cancelling a partial guide leaves the introduction eligible even if the request later resolves', async () => {
  await open('sidebar')
  await start()
  await page.evaluate(() => window.__test.emit(window.__test.requests[0].args.eventName, { delta: '部分回答', done: false }))
  await page.locator('.stop-btn').click()
  await page.evaluate(() => window.__test.requests[0].resolve('部分回答'))
  assert.deepEqual(await flags(), { experienced: null, dismissed: null })
  assert.ok(await page.evaluate(() => !!window.__test.cancelled))
})

test('a completed guide dismisses the introduction in other windows and after reopening', async () => {
  await open('intro')
  const second = await context.newPage()
  second.on('pageerror', error => errors.push(error.message))
  await second.goto(`${origin}/__guided-test?mode=standalone`)
  await second.waitForFunction(() => window.__test?.mounted)
  await second.locator('.guided-reading-btn').click()
  await second.waitForFunction(() => window.__test.requests.length === 1)
  await second.evaluate(() => window.__test.requests[0].resolve('这个小例子清楚吗？'))
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  await page.reload()
  await page.waitForFunction(() => window.__test?.mounted)
  assert.equal(await page.getByRole('dialog').count(), 0)
  await second.close()
})

test('intro and both header entries fit narrow, wide, light and dark layouts, with keyboard focus contained', async () => {
  await open()
  await page.setViewportSize({ width: 900, height: 780 })
  await page.getByRole('button', { name: '我知道了' }).focus()
  await page.keyboard.press('Tab')
  assert.ok(await page.getByRole('button', { name: '关闭', exact: true }).evaluate(el => document.activeElement === el))
  await page.keyboard.press('Shift+Tab')
  assert.ok(await page.getByRole('button', { name: '我知道了' }).evaluate(el => document.activeElement === el))
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/guided-intro.png` })
    await page.evaluate(() => document.documentElement.dataset.theme = 'dark')
    await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/guided-intro-dark.png` })
  }
  await page.setViewportSize({ width: 360, height: 640 })
  const introBounds = await page.getByRole('dialog').boundingBox()
  assert.ok(introBounds.x >= 0 && introBounds.x + introBounds.width <= 360)
  assert.ok(introBounds.y >= 0 && introBounds.y + introBounds.height <= 640)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/guided-intro-narrow.png` })
  for (const [mode, width, locale = 'zh'] of [['sidebar', 220], ['sidebar', 340], ['standalone', 400], ['standalone', 540], ['standalone', 800], ['standalone', 1100], ['standalone', 400, 'en'], ['standalone', 540, 'en']]) {
    await page.setViewportSize({ width, height: 780 })
    await open(mode, `&locale=${locale}`)
    assert.equal(await page.locator('.guided-reading-btn .ai-star').count(), 1)
    const button = await page.locator('.guided-reading-btn').boundingBox()
    const model = await page.locator('.model-trigger').boundingBox()
    assert.ok(button.x >= 0 && button.x + button.width <= width, `${mode} ${width}: button fits`)
    if (mode === 'standalone') {
      const controlsEnd = await page.locator('.header-actions > button:visible').last().boundingBox()
      assert.ok(controlsEnd.x + controlsEnd.width <= model.x, 'header actions do not overlap model picker')
    }
    else assert.ok(button.y + button.height <= model.y, 'sidebar guide is above model picker')
    if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/guided-${mode}-${width}-${locale}.png` })
    if (mode === 'standalone' && width < 700) {
      await page.getByTitle('对话历史', { exact: true }).click()
      await page.locator('.history-panel').waitFor()
      assert.ok(await page.locator('.history-panel .new-chat-btn').isVisible())
    }
  }
  assert.deepEqual(errors, [])
})
