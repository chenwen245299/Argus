// Real settings UI, local desktop mocks, no Exa account or library access.
// ARGUS_TEST_PLAYWRIGHT may point to a bundled Playwright index.mjs.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mock = `export const invoke = (command, args) => window.__test.invoke(command, args);`
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-exa-desktop', enforce: 'pre',
      resolveId: source => source === '@tauri-apps/api/core' ? '\0desktop' : null,
      load: id => id === '\0desktop' ? mock : null,
    }, vue()], optimizeDeps: { entries: ['src/components/settings/AgentSettings.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__exa-test')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>body { margin: 12px; } button { border: 0; } </style><script type="module">
      import { createApp } from 'vue'; import { createI18n } from 'vue-i18n';
      import AgentSettings from '/src/components/settings/AgentSettings.vue';
      import { zh, en } from '/src/i18n/locales.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      import { addCollection } from '@iconify/vue'; import fluent from '@iconify-json/fluent/icons.json'; addCollection(fluent);
      const params = new URLSearchParams(location.search);
      const defaults = { maxRounds: 50, keepCacheWarm: true, responseStyle: 'concise', systemPrompt: '',
        defaultSystemPrompt: 'Default prompt', servers: [], builtinToolCount: 0, minRounds: 1, maxRoundsLimit: 500, maxServers: 20 };
      const test = window.__test = { calls: [], settings: {enabled: false, hasKey: params.get('key') === 'yes', libraryRoot: '/mock-library'},
        fail: '', defer: false, pending: null,
        invoke(command, args) {
          test.calls.push({command, args});
          if (command === 'agent_get_settings' || command === 'agent_save_settings') return Promise.resolve(defaults);
          if (command === 'agent_list_builtin_tools') return Promise.resolve([]);
          if (command === 'exa_get_settings') return Promise.resolve({...test.settings});
          if (command === 'open_url') return Promise.resolve(null);
          if (command === 'exa_save_settings') {
            if (test.fail) return Promise.reject(test.fail);
            const save = () => {
              test.settings = { ...test.settings, enabled: args.clearKey ? false : args.enabled,
                hasKey: args.clearKey ? false : !!args.apiKey || test.settings.hasKey };
              return {...test.settings};
            };
            if (test.defer) return new Promise(resolve => test.pending = () => resolve(save()));
            return Promise.resolve(save());
          }
          throw new Error(command);
        },
      };
      createApp(AgentSettings).use(createI18n({legacy:false, locale:params.get('locale') || 'zh', messages:{zh,en}})).mount('#app');
      </script>`))
  })
  await server.listen(); origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true, timeout: 30_000 })
  page = await browser.newPage({ viewport: { width: 900, height: 1000 } })
  page.setDefaultTimeout(5_000)
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await server?.close() })
async function open(query = '') {
  await page.goto(`${origin}/__exa-test?${query}`)
  await page.waitForFunction(() => window.__test?.calls.some(call => call.command === 'exa_get_settings'))
  await page.waitForFunction(() => !document.querySelector('#exa-enabled').disabled)
}
const card = () => page.locator('.exa-settings')
const key = () => page.locator('#exa-api-key')
const save = () => card().locator('.exa-save')
const calls = () => page.evaluate(() => window.__test.calls.filter(call => call.command === 'exa_save_settings'))

test('the Agent settings tab contains Exa; enabling requires a key', async () => {
  await open()
  assert.equal(await card().getAttribute('aria-label'), 'Exa 联网搜索与网页读取')
  assert.equal(await key().getAttribute('type'), 'password')
  await page.locator('#exa-enabled').check()
  assert.equal(await save().isDisabled(), true)
  await key().fill('test-exa-credential')
  await save().click()
  await card().getByRole('status').waitFor()
  assert.deepEqual((await calls())[0].args, { enabled: true, apiKey: 'test-exa-credential', clearKey: false, expectedRoot: '/mock-library' })
  assert.equal(await key().inputValue(), '')
  assert.match(await key().getAttribute('placeholder'), /已保存密钥/)
})

test('blank saves keep a stored key, and removal clears it and disables Exa', async () => {
  await open('key=yes')
  await page.locator('#exa-enabled').check()
  await save().click()
  await card().getByRole('status').waitFor()
  assert.equal((await calls())[0].args.apiKey, null)
  await key().fill('unsaved-replacement')
  await card().getByRole('button', { name: '移除密钥并停用' }).click()
  await page.waitForFunction(() => !window.__test.settings.hasKey)
  assert.deepEqual((await calls()).at(-1).args, { enabled: true, apiKey: null, clearKey: true, expectedRoot: '/mock-library' })
  assert.equal(await page.locator('#exa-enabled').isChecked(), false)
  assert.equal(await key().inputValue(), '')
})

test('pending saves prevent duplicate submissions and failures preserve the draft', async () => {
  await open()
  await key().fill('test-exa-credential')
  await page.evaluate(() => window.__test.defer = true)
  await save().click()
  assert.equal(await key().isDisabled(), true)
  assert.equal(await save().isDisabled(), true)
  await page.evaluate(() => window.__test.pending())
  await card().getByRole('status').waitFor()
  await key().fill('replacement-credential')
  await page.evaluate(() => { window.__test.defer = false; window.__test.fail = '文献库已切换，请重新打开 Exa 设置。' })
  await save().click()
  assert.match(await card().getByRole('alert').innerText(), /文献库已切换/)
  assert.equal(await key().inputValue(), 'replacement-credential')
})

test('English copy, dashboard link and narrow layout work', async () => {
  await page.setViewportSize({ width: 400, height: 1000 })
  await open('locale=en')
  assert.equal(await card().getAttribute('aria-label'), 'Exa web search and page reading')
  await card().getByRole('button', { name: 'Get an API key' }).click()
  const link = await page.evaluate(() => window.__test.calls.find(call => call.command === 'open_url'))
  assert.equal(link.args.url, 'https://dashboard.exa.ai/api-keys')
  assert.equal(await card().evaluate(el => el.scrollWidth <= el.clientWidth), true)
})

test('the settings produce no browser errors', () => assert.deepEqual(errors, []))
