// Real update store + prompt, isolated desktop/updater mocks; no install or internet.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (command, args) => window.__test.invoke(command, args);`,
  '@tauri-apps/api/app': `export const getVersion = async () => { window.__test.versionReads++; return '0.109.0'; };`,
  '@tauri-apps/api/event': `export const listen = async (name, handler) => {
    const handlers = window.__test.listeners.get(name) || new Set();
    window.__test.listeners.set(name, handlers); handlers.add(handler);
    return () => handlers.delete(handler);
  };`,
  '@tauri-apps/plugin-updater': `export const check = () => {
    window.__test.checks++;
    if (window.__test.checkError) return Promise.reject(new Error(window.__test.checkError));
    return window.__test.holdCheck ? new Promise(resolve => window.__test.pendingCheck = resolve) : Promise.resolve(window.__test.offer);
  };`,
  '@tauri-apps/plugin-process': `export const relaunch = async () => window.__test.relaunched = true;`,
}
before(async () => {
  server = await createServer({
    configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-updater', enforce: 'pre',
      resolveId: source => source in mocks ? '\0mock:' + source : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null,
    }, vue()],
    optimizeDeps: { entries: ['src/components/UpdatePrompt.vue'] },
    server: { host: '127.0.0.1', port: 0, strictPort: false }, logLevel: 'error',
  })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__update-test')) return next()
    const html = `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <script type="module">
      import { createApp, h, ref } from 'vue'; import { createI18n } from 'vue-i18n'; import { createPinia } from 'pinia';
      import { addCollection } from '@iconify/vue'; import fluent from '@iconify-json/fluent/icons.json';
      import UpdatePrompt from '/src/components/UpdatePrompt.vue';
      import AboutSettings from '/src/components/settings/AboutSettings.vue';
      import { useSettingsStore } from '/src/stores/settings.ts';
      import { useNativeUpdateMenu } from '/src/composables/useNativeUpdateMenu.ts';
      import * as store from '/src/stores/update.ts';
      import { zh, en } from '/src/i18n/locales.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      const test = window.__test = { ...store, requests: [], checks: 0, versionReads: 0,
        listeners: new Map(), aboutOpened: 0,
        pendingNativeCheck: new URLSearchParams(location.search).has('pending'),
        emitNativeCheck() { for (const handler of test.listeners.get('argus-check-for-updates') || []) handler({ payload: null }); },
        requestNativeCheck() { test.pendingNativeCheck = true; test.emitNativeCheck(); },
        invoke(command, args) {
          if (command === 'take_update_check_request') { const pending = test.pendingNativeCheck; test.pendingNativeCheck = false; return Promise.resolve(pending); }
          if (command === 'get_library_size') return Promise.reject('No library open');
          if (command === 'get_update_release_notes') return new Promise((resolve, reject) => test.requests.push({ args, resolve, reject }));
          return Promise.resolve();
        },
        offer: { version: '0.115.0', body: '- 新增 AI 领读\\n- 完善 SoCLaaS 支持',
          downloadAndInstall: callback => { test.downloadCallback = callback; return new Promise(resolve => test.downloadDone = resolve); } },
      };
      addCollection(fluent);
      const app = createApp({ setup() {
        const showAbout = ref(false);
        useNativeUpdateMenu(() => { test.aboutOpened++; showAbout.value = true; });
        return () => h('main', [showAbout.value ? h(AboutSettings) : null, h(UpdatePrompt)]);
      }});
      app.use(createPinia()).use(createI18n({ legacy: false,
        locale: new URLSearchParams(location.search).get('locale') || 'zh', messages: { zh, en } }));
      test.settings = useSettingsStore(); test.settings.settings.auto_check_updates = false;
      test.unmount = () => app.unmount(); app.mount('#app');
      test.mounted = true;
      </script>`
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, html))
  })
  await server.listen()
  origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 960, height: 850 } })
  page.on('pageerror', error => { errors.push(error.message); console.error(error.message) })
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await server?.close() })
async function open(locale = 'zh', pending = false) {
  await page.goto(`${origin}/__update-test?locale=${locale}${pending ? '&pending=1' : ''}`)
  await page.waitForFunction(() => window.__test?.mounted)
}
const history = Array.from({ length: 6 }, (_, index) => ({
  version: `0.${115 - index}.0`, published_at: `2026-10-${String(13 - index).padStart(2, '0')}T01:00:00Z`,
  body: ['- 新增 AI 领读\n- 完善 SoCLaaS 支持', '- arXiv 论文暂存\n- 支持拖动分类',
    '- 改进公式渲染', '- 增加工具调用\n- 修复阅读状态', '- 改进论文搜索', '- 修复 PDF 显示'][index],
}))
async function resolveHistory(entries = history, index = -1) {
  await page.evaluate(({ entries, index }) => window.__test.requests.at(index).resolve(entries), { entries, index })
  await page.waitForFunction(() => window.__test.updateStore.releaseHistoryState === 'ready')
}
const sections = () => page.locator('.rn-release-heading h3').allTextContents()

test('startup reads the installed version and immediately shows latest notes while six missed releases load in the background', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  assert.deepEqual(await page.evaluate(() => window.__test.requests[0].args), { currentVersion: '0.109.0', targetVersion: '0.115.0' })
  assert.equal(await page.evaluate(() => window.__test.versionReads), 1)
  assert.ok(await page.getByText('正在加载你错过的版本…').isVisible())
  assert.ok(await page.getByRole('button', { name: '立即更新' }).isVisible())
  assert.deepEqual(await sections(), ['v0.115.0'])
  await resolveHistory()
  assert.deepEqual(await sections(), history.map(entry => 'v' + entry.version))
  assert.ok(await page.getByText('共 6 个版本', { exact: true }).isVisible())
  assert.equal(await page.locator('.rn-release').count(), 6)
  assert.equal(await page.locator('.rn-release-heading time').last().innerText(), '2026-10-08')
  assert.equal(await page.evaluate(() => window.__test.updateStore.state), 'available')
})

test('a two-release jump includes only the backend range and falls back to the manifest for an empty or absent newest body', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(true))
  await resolveHistory([{ ...history[0], body: '' }, history[1]])
  assert.deepEqual(await sections(), ['v0.115.0', 'v0.114.0'])
  assert.ok(await page.getByText('共 2 个版本', { exact: true }).isVisible())
  assert.ok(await page.getByText('新增 AI 领读', { exact: true }).isVisible())
})

test('history failures retain the latest notes and install button, and retry succeeds without checking or changing the offered installer', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await page.evaluate(() => window.__test.requests[0].reject(new Error('rate limited')))
  await page.getByText('较早版本的更新内容暂时无法加载。').waitFor()
  assert.deepEqual(await sections(), ['v0.115.0'])
  assert.ok(await page.getByRole('button', { name: '立即更新' }).isVisible())
  assert.equal(await page.evaluate(() => window.__test.updateStore.state), 'available')
  await page.getByRole('button', { name: '重试', exact: true }).click()
  await page.waitForFunction(() => window.__test.requests.length === 2)
  await resolveHistory(history.slice(1))
  assert.deepEqual(await sections(), history.map(entry => 'v' + entry.version))
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
})

test('nearby checks reuse the history cache and a simultaneous manual check shares the updater request while still surfacing the dialog', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await resolveHistory()
  await page.evaluate(() => { window.__test.updateStore.showReleaseNotes = false; return window.__test.checkForUpdates(false) })
  assert.equal(await page.getByRole('dialog').count(), 0)
  assert.equal(await page.evaluate(() => window.__test.requests.length), 1)
  await page.evaluate(() => {
    window.__test.holdCheck = true;
    void window.__test.checkForUpdates(false);
    void window.__test.checkForUpdates(true);
  })
  assert.equal(await page.evaluate(() => window.__test.checks), 3)
  await page.evaluate(() => window.__test.pendingCheck(window.__test.offer))
  await page.getByRole('dialog').waitFor()
  assert.equal(await page.evaluate(() => window.__test.requests.length), 1)
})

test('late history from a superseded offer or a no-update result cannot replace the active notes', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await page.evaluate(() => { window.__test.offer.version = '0.116.0'; return window.__test.checkForUpdates(false) })
  await resolveHistory([{ version: '0.116.0', body: 'New offer', published_at: null }], 1)
  await page.evaluate(entries => window.__test.requests[0].resolve(entries), history)
  assert.deepEqual(await sections(), ['v0.116.0'])
  await page.evaluate(() => { void window.__test.loadUpdateReleaseNotes(true); window.__test.offer = null; return window.__test.checkForUpdates(false) })
  await page.evaluate(entries => window.__test.requests.at(-1).resolve(entries), history)
  assert.deepEqual(await page.evaluate(() => window.__test.updateStore.releaseHistory), [])
  assert.equal(await page.evaluate(() => window.__test.updateStore.hasUpdate), false)
})

test('install starts while history is loading, and later notes do not reset download progress or the updater state', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await page.getByRole('button', { name: '立即更新' }).click()
  await page.evaluate(() => {
    window.__test.downloadCallback({ event: 'Started', data: { contentLength: 100 } });
    window.__test.downloadCallback({ event: 'Progress', data: { chunkLength: 42 } });
  })
  await resolveHistory()
  assert.equal(await page.evaluate(() => window.__test.updateStore.state), 'downloading')
  assert.equal(await page.evaluate(() => window.__test.updateStore.downloadProgress), 42)
  assert.ok(await page.locator('.rn-progress-label').isVisible())
  await page.evaluate(() => window.__test.downloadDone())
  await page.waitForFunction(() => window.__test.relaunched)
})

test('each release body is sanitized and empty older notes receive a neutral placeholder', async () => {
  await open()
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await resolveHistory([{ ...history[0], body: '<img src=x onerror="window.pwn=1"><script>window.pwn=1</script>\n\n- Safe note' }, { ...history[1], body: '' }])
  assert.equal(await page.locator('.rn-body [onerror], .rn-body script').count(), 0)
  assert.equal(await page.evaluate(() => window.pwn), undefined)
  assert.ok(await page.getByText('此版本未提供更新说明。').isVisible())
})

test('all versions remain expanded in a scrollable body with the install footer visible on narrow and dark layouts', async () => {
  await open()
  await page.setViewportSize({ width: 960, height: 850 })
  await page.evaluate(() => window.__test.checkForUpdates(false))
  await resolveHistory()
  const scrolling = await page.locator('.rn-body').evaluate(el => el.scrollHeight > el.clientHeight)
  assert.ok(scrolling)
  assert.equal(await page.locator('.rn-release').count(), 6)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/update-six-releases.png` })
    await page.evaluate(() => document.documentElement.dataset.theme = 'dark')
    await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/update-six-releases-dark.png` })
  }
  await page.setViewportSize({ width: 380, height: 650 })
  const bounds = await page.getByRole('dialog').boundingBox()
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= 380)
  const install = await page.getByRole('button', { name: '立即更新' }).boundingBox()
  assert.ok(install.y + install.height < 650)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/update-six-releases-narrow.png` })
  assert.deepEqual(errors, [])
})

test('a native menu request queued before mount opens About and the update prompt once, even without a library or automatic checks', async () => {
  await open('en', true)
  await page.locator('.about-settings').waitFor()
  await page.getByRole('dialog').waitFor()
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
  assert.equal(await page.evaluate(() => window.__test.aboutOpened), 1)
  await page.evaluate(() => window.__test.emitNativeCheck())
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
  await resolveHistory()
  await page.evaluate(() => window.__test.unmount())
  assert.equal(await page.evaluate(() => window.__test.listeners.get('argus-check-for-updates').size), 0)
})

test('native menu checks show current-version and failure feedback in About without requiring a library', async () => {
  await open()
  await page.evaluate(() => { window.__test.offer = null; window.__test.requestNativeCheck() })
  await page.locator('.about-settings .latest-tip').waitFor()
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
  assert.equal(await page.getByRole('dialog').count(), 0)
  await page.evaluate(() => { window.__test.checkError = 'Offline'; window.__test.requestNativeCheck() })
  await page.locator('.about-settings .error-msg').getByText('检查失败：Offline').waitFor()
  assert.equal(await page.evaluate(() => window.__test.checks), 2)
})

test('repeated native menu clicks share an active check and do not start another check or install during download', async () => {
  await open()
  await page.evaluate(() => { window.__test.holdCheck = true; window.__test.requestNativeCheck() })
  await page.locator('.about-settings .btn-check[disabled]').waitFor()
  await page.evaluate(() => window.__test.requestNativeCheck())
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
  await page.evaluate(() => window.__test.pendingCheck(window.__test.offer))
  await page.getByRole('dialog').waitFor()
  await page.getByRole('button', { name: '立即更新', exact: true }).click()
  await page.locator('.rn-progress-wrap').waitFor()
  await page.evaluate(() => window.__test.requestNativeCheck())
  assert.equal(await page.evaluate(() => window.__test.checks), 1)
  assert.equal(await page.evaluate(() => window.__test.updateStore.state), 'downloading')
  assert.deepEqual(errors, [])
})
