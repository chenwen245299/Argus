// SoCLaaS controls in real paper/sidebar/library composers, with local desktop mocks.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (command, args) => window.__test?.invoke(command, args) ?? Promise.resolve(null);
    export const convertFileSrc = path => path; export const isTauri = () => false;`,
  '@tauri-apps/api/event': `export const listen = async (name, callback) => {
    if (!window.__test) return () => {};
    const callbacks = window.__test.listeners.get(name) || new Set();
    window.__test.listeners.set(name, callbacks); callbacks.add(callback); return () => callbacks.delete(callback);
  }; export const emit = async (name, payload) => window.__test?.emit(name, payload);
     export const emitTo = async (target, name, payload) => window.__test?.emit(name, payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow = () => ({ label: 'main',
    onResized: async () => () => {}, onFocusChanged: async () => () => {} });
    export class Window { static async getByLabel() { return null; } }`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow = () => ({ label: 'main' });
    export class WebviewWindow { static async getByLabel() { return null; } }`,
}
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
  plugins: [{ name: 'mock-desktop', enforce: 'pre',
    resolveId: source => source in mocks ? '\0mock:' + source : null,
    load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null,
  }, vue()], optimizeDeps: { entries: ['src/components/tabs/AiTab.vue', 'src/components/LibraryChat.vue'] },
  server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__soclaas-test')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><html><meta charset="utf-8"><body><div id="app"></div>
      <style>body{margin:0}#app{height:100vh}.test-panel{height:100vh;width:500px}.lc-root{height:100vh}</style>
      <script type="module">
      import { createApp, h, ref } from 'vue';
      import { createPinia } from 'pinia';
      import { createI18n } from 'vue-i18n';
      import { addCollection } from '@iconify/vue';
      import fluent from '@iconify-json/fluent/icons.json';
      import AiTab from '/src/components/tabs/AiTab.vue';
      import LibraryChat from '/src/components/LibraryChat.vue';
      import AiSettings from '/src/components/settings/AiSettings.vue';
      import { useAiStore } from '/src/stores/ai.ts';
      import { useSettingsStore } from '/src/stores/settings.ts';
      import { useThinkingStore } from '/src/stores/thinking.ts';
      import { zh, en } from '/src/i18n/locales.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      addCollection(fluent);
      const mode = new URLSearchParams(location.search).get('mode'), slug = ref('paper');
      const test = window.__test = { requests: [], calls: [], listeners: new Map(), saved: [], cancelAuto: true,
        emit(name, payload) { for (const cb of test.listeners.get(name) || []) cb({ payload }); },
        switchPaper: value => slug.value = value,
        finish(index, text='finished') { const r=test.requests[index]; r.finished=true; r.resolve(text); },
        invoke(command, args) {
          test.calls.push({command,args});
          if (command === 'get_current_library') return Promise.resolve('/mock-library');
          if (command === 'fetch_provider_models') return Promise.resolve(test.catalogue);
          if (command === 'get_ai_settings') return Promise.resolve(JSON.parse(JSON.stringify(test.aiSettings)));
          if (command === 'get_paper_meta') return Promise.resolve({title:'Example paper',abstract:'A paper'});
          if (['get_paper_ai_conversations','get_paper_chat','get_library_conversations','list_papers'].includes(command)) return Promise.resolve([]);
          if (command.startsWith('save_')) test.saved.push({command,args:JSON.parse(JSON.stringify(args))});
          if (command === 'chat_with_library') return new Promise((resolve,reject) => test.requests.push({args,resolve,reject}));
          if (command === 'cancel_ai_request' && test.cancelAuto) {
            const r = test.requests.find(r => r.args.requestId === args.requestId);
            if (r && !r.finished) { r.finished=true; r.reject('cancelled'); }
          }
          return Promise.resolve(null);
        },
      };
      const app = createApp({setup(){return () => mode === 'settings' ? h(AiSettings) : mode === 'library' ? h(LibraryChat)
        : h('div',{class:'test-panel'},[h(AiTab,{slug:slug.value,standalone:mode === 'standalone'})]);}});
      const pinia=createPinia(); app.use(pinia);
      app.use(createI18n({legacy:false,locale:'zh',messages:{zh,en}}));
      const ai=useAiStore(pinia); ai.loaded=true; test.ai=ai;
      ai.settings={default_provider_id:'provider',default_model_id:'test-vision',providers:[{
        id:'provider',name:'Example',enabled:true,kind:'custom',base_url:'https://example.test/v1',
        models:[{id:'test-vision',display_name:'Test vision',capabilities:['vision','tool_calling']}],
      }]};
      test.aiSettings=JSON.parse(JSON.stringify(ai.settings));
      ai.load=async()=>{};
      useSettingsStore(pinia).load=async()=>{};
      test.thinking = useThinkingStore(pinia);
      test.setModelMetadata = (efforts, kind='soclaas') => {
        const provider = ai.settings.providers[0];
        provider.kind = kind;
        provider.base_url = kind === 'soclaas' ? 'https://soclaas-api.comp.nus.edu.sg/v1' : 'https://example.test/v1';
        provider.models[0].reasoning_efforts = efforts;
      };
      app.mount('#app'); test.mounted=true;
      </script></body></html>`))
  })
  await server.listen(); origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  const context = await browser.newContext({ viewport: { width: 1100, height: 840 } })
  await context.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
  page = await context.newPage(); page.on('pageerror', e => { errors.push(e.message); console.error('Browser error:', e.message) })
})
after(async () => { await browser?.close(); await server?.close() })
async function open(mode) {
  await page.goto(`${origin}/__soclaas-test?mode=${mode}`)
  await page.waitForFunction(() => window.__test?.mounted, null, { timeout: 10000 }).catch(async error => {
    console.error(await page.evaluate(() => ({ html: document.body.innerHTML.slice(0, 200), calls: window.__test?.calls, errors: window.__test?.errors })))
    throw error
  })
  if (mode === 'settings') return
  await page.locator('.composer textarea').waitFor()
  await page.waitForFunction(() => window.__test.calls.some(c => c.command === 'list_papers' || c.command === 'get_paper_meta'))
}
const composer = () => page.locator('.composer textarea').last()
async function send(text, key = 'Enter') { await composer().fill(text); await composer().press(key) }
const count = n => page.waitForFunction(n => window.__test.requests.length === n, n)
const finish = (index, text) => page.evaluate(({index,text}) => window.__test.finish(index,text), {index,text})

for (const mode of ['sidebar', 'standalone', 'library']) {
  test(`${mode}: exact discovered efforts are selectable, persisted and sent`, async () => {
    await open(mode)
    await page.evaluate(() => {
      window.__test.setModelMetadata(['max', 'high', 'low'])
      window.__test.thinking.useReasoning = true
    })
    await page.locator('.reasoning-picker > button').click()
    assert.deepEqual(await page.locator('.reasoning-levels button').allTextContents(), ['low', 'high', 'max'])
    await page.locator('.reasoning-levels button').getByText('max', { exact: true }).click()
    assert.equal(await page.locator('.reasoning-badge').innerText(), 'max')
    assert.match(await page.locator('.reasoning-hint').innerText(), /最低档 low/)
    await page.locator('.reasoning-picker > button').click()
    await send('Use the discovered maximum effort')
    await count(1)
    const request = await page.evaluate(() => window.__test.requests[0].args)
    assert.equal(request.reasoningEffort, 'max')
    assert.equal(request.useReasoning, true)
    assert.equal(request.modelId, 'test-vision')
    await finish(0, 'Completed answer')
    await page.locator('.stop-btn').waitFor({ state: 'hidden' })
    assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('argus:thinking-preference')).level), 'max')

    // Operators can repoint this same public alias. Existing drafts and the
    // saved preference survive; only supported choices and their badge change.
    await composer().fill('Preserve this draft')
    await page.evaluate(() => window.__test.setModelMetadata(['xhigh', 'medium', 'low', 'none']))
    await page.locator('.reasoning-picker > button').click()
    assert.deepEqual(await page.locator('.reasoning-levels button').allTextContents(), ['low', 'medium', 'xhigh'])
    assert.equal(await page.locator('.reasoning-badge').innerText(), 'xhigh')
    assert.equal(await page.locator('.reasoning-hint').count(), 0)
    await page.locator('.reasoning-levels button').getByText('medium', { exact: true }).click()
    await page.locator('.reasoning-picker > button').click()
    assert.equal(await composer().inputValue(), 'Preserve this draft')
    await send('Use the new alias configuration')
    await count(2)
    assert.equal(await page.evaluate(() => window.__test.requests[1].args.reasoningEffort), 'medium')
    await finish(1, 'Second answer')
    await page.locator('.stop-btn').waitFor({ state: 'hidden' })
  })

  test(`${mode}: five levels fit, missing controls stay honest, and other providers receive valid levels`, async () => {
    await open(mode)
    if (mode === 'sidebar') await page.locator('.test-panel').evaluate(el => el.style.width = '350px')
    await page.evaluate(() => {
      window.__test.setModelMetadata(['max', 'xhigh', 'high', 'medium', 'low'])
      window.__test.thinking.useReasoning = true
      window.__test.thinking.reasoningLevel = 'xhigh'
    })
    await page.locator('.reasoning-picker > button').click()
    await page.waitForFunction(() => getComputedStyle(document.querySelector('.reasoning-popover')).opacity === '1')
    assert.deepEqual(await page.locator('.reasoning-levels button').allTextContents(), ['low', 'medium', 'high', 'xhigh', 'max'])
    const popover = await page.locator('.reasoning-popover').boundingBox()
    assert.ok(popover.x >= 0 && popover.x + popover.width <= (mode === 'sidebar' ? 350 : 1100), 'all effort choices remain inside the window')
    assert.equal(await page.locator('.reasoning-levels button.active').innerText(), 'xhigh')
    if (process.env.ARGUS_TEST_OUTPUT_DIR) {
      await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
      await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/${mode}-soclaas-efforts.png` })
    }
    await page.locator('.reasoning-toggle').click()
    assert.equal(await page.locator('.reasoning-levels').count(), 0)
    await page.locator('.reasoning-picker > button').click()
    await send('Use the lowest effort when disabled')
    await count(1)
    assert.equal(await page.evaluate(() => window.__test.requests[0].args.useReasoning), false)
    await finish(0, 'Disabled answer')
    await page.locator('.stop-btn').waitFor({ state: 'hidden' })

    await page.evaluate(() => window.__test.setModelMetadata([]))
    await page.locator('.reasoning-picker > button').click()
    assert.equal(await page.locator('.reasoning-toggle').isDisabled(), true)
    assert.equal(await page.locator('.reasoning-levels button').count(), 0)
    assert.match(await page.locator('.reasoning-hint').innerText(), /未声明/)

    await page.evaluate(() => {
      window.__test.setModelMetadata(undefined, 'custom')
      window.__test.thinking.useReasoning = true
      window.__test.thinking.reasoningLevel = 'max'
    })
    assert.deepEqual(await page.locator('.reasoning-levels button').allTextContents(), ['低', '中', '高'])
    assert.equal(await page.locator('.reasoning-levels button.active').innerText(), '高')
    await page.locator('.reasoning-picker > button').click()
    await send('Switch back to a standard provider')
    await count(2)
    assert.equal(await page.evaluate(() => window.__test.requests[1].args.reasoningEffort), 'high')
    await finish(1, 'Standard answer')
    await page.locator('.stop-btn').waitFor({ state: 'hidden' })
  })
}



test('refetching an all-existing catalogue can apply effort/price/context updates while preserving edits', async () => {
  await open('settings')
  await page.evaluate(() => {
    window.__test.setModelMetadata(['xhigh', 'medium', 'low'])
    const model = window.__test.ai.settings.providers[0].models[0]
    Object.assign(model, { display_name: 'My model label', input_price_per_million: 9 })
    window.__test.catalogue = [{ ...model, display_name: 'Provider label', capabilities: ['vision'],
      reasoning_efforts: ['max', 'high', 'low'], context_length: 1048576,
      input_price_usd_per_million: 0.157332, output_price_usd_per_million: 0.511989 }]
  })
  await page.locator('.provider-item').first().click()
  await page.getByRole('button', { name: '获取模型列表', exact: true }).click()
  const update = page.getByRole('button', { name: '更新已有模型', exact: true })
  await update.waitFor()
  assert.equal(await update.isEnabled(), true)
  await update.click()
  await page.waitForFunction(() => window.__test.calls.some(c => c.command === 'update_ai_provider'))
  const saved = await page.evaluate(() => window.__test.calls.find(c => c.command === 'update_ai_provider').args.provider.models[0])
  assert.deepEqual(saved.reasoning_efforts, ['max', 'high', 'low'])
  assert.equal(saved.context_length, 1048576)
  assert.equal(saved.input_price_usd_per_million, 0.157332)
  assert.equal(saved.display_name, 'My model label')
  assert.equal(saved.input_price_per_million, 9)
  assert.deepEqual(saved.capabilities, ['vision', 'tool_calling'])
})

test('no browser runtime errors', () => assert.deepEqual(errors, []))
