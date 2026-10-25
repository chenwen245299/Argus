// Local browser regression: real Vue panels and intro, with desktop I/O mocked.
// ARGUS_TEST_PLAYWRIGHT may point to a bundled Playwright index.mjs.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, readFile } from 'node:fs/promises'
import ts from 'typescript'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const promptSource = await readFile(new URL('../src/utils/guidedReading.ts', import.meta.url), 'utf8')
// Keep prior shipped defaults private in production, but load their exact
// values here to verify migration without duplicating the historical prompts.
const shipped = await import('data:text/javascript;base64,' + Buffer.from(ts.transpileModule(
  promptSource + '\nexport { LEGACY_PROMPT_ZH, LEGACY_PROMPT_EN, LEGACY_PROMPT_ZH_V2, LEGACY_PROMPT_EN_V2, LEGACY_PROMPT_ZH_V3, LEGACY_PROMPT_EN_V3 };',
  { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext } },
).outputText).toString('base64'))
const legacyPresets = { zh: shipped.LEGACY_PROMPT_ZH, en: shipped.LEGACY_PROMPT_EN,
  'zh-v2': shipped.LEGACY_PROMPT_ZH_V2, 'en-v2': shipped.LEGACY_PROMPT_EN_V2,
  'zh-v3': shipped.LEGACY_PROMPT_ZH_V3, 'en-v3': shipped.LEGACY_PROMPT_EN_V3 }
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
      import QaSettings from '/src/components/settings/QaSettings.vue';
      import { useAiStore } from '/src/stores/ai.ts';
      import { useSettingsStore } from '/src/stores/settings.ts';
      import { zh, en } from '/src/i18n/locales.ts';
      import * as guided from '/src/utils/guidedReading.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      addCollection(fluent);
      const query = new URLSearchParams(location.search), mode = query.get('mode') || 'intro';
      const legacyPresets = ${JSON.stringify(legacyPresets)};
      const legacy = query.get('legacy');
      const savedGuide = legacy ? [{id:'legacy-guide',slug:'paper',title:'AI 领读',source:'guidedReading',
        createdAt:'2026-01-01T00:00:00Z',updatedAt:'2026-01-01T00:00:00Z',nodes:[
          {id:'legacy-question',role:'user',content:legacyPresets[legacy],createdAt:'2026-01-01T00:00:00Z'},
          {id:'legacy-group',role:'assistantGroup',promptId:'legacy-question',createdAt:'2026-01-01T00:00:00Z',answers:[
            {id:'legacy-answer',providerId:'provider',providerName:'Example',modelId:'test-vision',modelName:'Test vision',
              content:'Earlier explanation.',streaming:false,createdAt:'2026-01-01T00:00:00Z'}]},
        ]}] : [];
      const test = window.__test = { requests: [], saved: [], listeners: new Map(), guided, legacyPresets, root:'/mock-library', saveError:false,
        emit(name, payload) { for (const cb of test.listeners.get(name) || []) cb({ payload }); },
        invoke(command, args) {
          if (command === 'get_current_library') { test.rootReads=(test.rootReads||0)+1;return Promise.resolve(test.root); }
          if (command === 'get_ai_settings') return Promise.resolve(JSON.parse(JSON.stringify(test.aiSettings)));
          if (command === 'get_guided_reading_settings') {
            const config=JSON.parse(localStorage.getItem(test.root+':guides')||'{"presets":[],"defaultId":"builtin"}');
            if(test.holdGuides)return new Promise(resolve=>test.pendingGuideRead={resolve,config});
            return Promise.resolve(config);
          }
          if (command === 'save_guided_reading_settings') {
            if(test.saveError || args.expectedRoot!==test.root)return Promise.reject(new Error('save failed'));
            localStorage.setItem(test.root+':guides',JSON.stringify(args.settingsData));
            test.emit('guided-reading-settings-changed',{root:test.root});return Promise.resolve();
          }
          if (command === 'get_paper_meta') return Promise.resolve({ title: 'Example paper', abstract: 'A complete paper' });
          if (command === 'get_paper_ai_conversations') return Promise.resolve(structuredClone(savedGuide));
          if (command === 'get_paper_chat') return Promise.resolve([]);
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
        mode === 'settings' ? h(QaSettings, {initialTab:'guided-reading'}) : mode !== 'intro' ? h(AiTab, { slug: 'paper', standalone: mode === 'standalone' }) : null,
        showIntro.value ? h(Intro, { ready: introReady.value, key: introKey.value }) : null,
      ]); } });
      const pinia = createPinia(); app.use(pinia);
      app.use(createI18n({ legacy: false, locale: query.get('locale') || 'zh', messages: { zh, en } }));
      const ai = useAiStore(pinia); ai.loaded = true;
      ai.settings = { default_provider_id: 'provider', default_model_id: 'test-vision', providers: [{
        id: 'provider', name: 'Example', enabled: true, kind: 'custom', base_url: 'https://example.test/v1',
        models: [{ id: 'test-vision', display_name: 'Test vision', capabilities: ['vision', 'tool_calling'] }],
      }] };
      test.aiSettings=JSON.parse(JSON.stringify(ai.settings));
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
  else if(mode==='settings')await page.locator('.reading-settings textarea').waitFor()
  else await page.locator('.guided-reading-btn').waitFor({ state: 'visible' })
}
async function start() {
  await page.locator('.guided-reading-btn').click()
  await page.waitForFunction(() => window.__test.requests.length > 0)
  assert.equal(await page.evaluate(() => window.__test.requests.at(-1).args.learnUserProfile), false)
}
async function finish(text = '先想象一个走迷宫的机器人。它保留几条可能的路线，走一段再判断，就能避免过早选错。') {
  await page.evaluate(text => window.__test.requests.at(-1).resolve(text), text)
  await page.locator('.stop-btn').waitFor({ state: 'hidden' })
}
async function flags() {
  return page.evaluate(() => ({
    experienced: localStorage.getItem(window.__test.guided.GUIDED_READING_EXPERIENCED_KEY),
    dismissed: localStorage.getItem(window.__test.guided.GUIDED_READING_DISMISSED_KEY),
  }))
}

test('both defaults follow the research order, teach just in time, annotate by fixed colors and stop after one small step', async () => {
  await open()
  const prompts = await page.evaluate(() => ['zh', 'en'].map(locale => window.__test.guided.guidedReadingPrompt(locale)))
  for (const prompt of prompts) {
    assert.match(prompt, /完整正文|complete text/)
    assert.match(prompt, /图表必须实际查看|Actually view the figures and tables/)
    assert.match(prompt, /附录|appendices/)
    assert.match(prompt, /背景与任务 → 现有方法及其问题 → 作者的核心思路与整体方法 → 评测设计 → 结果与结论|background and task → existing methods and their problems → the authors' core idea and overall method → evaluation design → results and conclusions/)
    assert.match(prompt, /不要把“先问两个问题”当作固定开场|Do not treat "ask two questions first" as a fixed opening/)
    assert.match(prompt, /每轮最多引入一个新的核心概念|at most one new core concept per turn/)
    assert.match(prompt, /100–250/)
    assert.match(prompt, /即使我说自己不熟悉，也不要马上集中补课|Even if I say I am unfamiliar with something, do not immediately give me a background course/)
    assert.match(prompt, /黄色：Method 的重点部分|Yellow: the key parts of the Method/)
    assert.match(prompt, /绿色：段落总结与理解说明|Green: paragraph summaries and explanations/)
    assert.match(prompt, /紫色：我明确指定的最重要创新点|Purple: the most important innovations I explicitly designate/)
    assert.match(prompt, /红色：论文明确指出的方法问题与局限|Red: problems and limitations of methods that the paper explicitly states/)
    assert.match(prompt, /蓝色：需要以后深入了解的内容|Blue: material to study in more depth later/)
    assert.match(prompt, /内容去重和选区避让|Content de-duplication and selection avoidance/)
    assert.match(prompt, /写入后检查，重试前核实|Check after writing; verify before retrying/)
    assert.match(prompt, /不必逐次询问|do not ask for permission each time/)
    assert.match(prompt, /伪代码和最小实现|pseudocode and a minimal implementation/)
    assert.match(prompt, /每一行有实际代码的内容都要配中文注释|Every line of actual code you show or provide must carry a Chinese comment/)
    assert.match(prompt, /我说“继续”，才推进|only when I say "continue"/)
    assert.match(prompt, /不必每轮测试我|Do not test me every turn/)
    assert.match(prompt, /首次回复简短说明实际阅读范围和任何缺失，然后从一个具体场景开始|first reply, briefly state what you actually read and anything missing, then start from a concrete scenario/)
    assert.doesNotMatch(prompt, /每轮末尾只问|End each turn with one|Do not create notes, annotations|不要自动创建笔记、批注|再问一两个了解我起点的问题|then ask one or two questions/)
    assert.ok(prompt.includes('$...$') && prompt.includes('$$...$$'))
    // LaTeX in the examples survives the template literal: backslashes, not tabs or line breaks.
    assert.ok(prompt.includes('`$\\nabla_x E_\\theta(x)$`') && prompt.includes('`\\n`'))
  }
  assert.ok(prompts[0].startsWith('请担任我的交互式论文导师，带我一步一步读懂论文'))
  assert.ok(prompts[0].endsWith('不要一次输出整篇总结或集中补充背景知识。'))
})

test('request upgrades preserve stored presets, assistant quotes, custom messages and attachment data', async () => {
  await open()
  const result = await page.evaluate(() => {
    const { guided, legacyPresets } = window.__test
    const attachment = {type:'image_url',image_url:{url:'data:image/png;base64,test'}}
    const history = [
      {role:'user',content:legacyPresets.zh},
      {role:'assistant',content:legacyPresets.zh},
      {role:'user',content:[{type:'text',text:legacyPresets.en},attachment]},
      {role:'user',content:legacyPresets.zh + '\nMy custom addition.'},
      {role:'user',content:'继续'},
    ]
    const before = JSON.stringify(history)
    const updated = guided.guidedReadingRequestHistory(history)
    return {
      unchanged: JSON.stringify(history) === before,
      zh: updated[0].content === guided.guidedReadingPrompt('zh'),
      en: updated[2].content[0].text === guided.guidedReadingPrompt('en'),
      quote: updated[1] === history[1], attachment: updated[2].content[1] === attachment,
      custom: updated[3] === history[3], normal: updated[4] === history[4],
      recognizable: Object.values(legacyPresets).every(prompt => guided.isGuidedReadingPrompt(prompt)),
      v2: ['zh','en'].every(locale => guided.guidedReadingRequestHistory([{role:'user',content:legacyPresets[locale+'-v2']}])[0].content === guided.guidedReadingPrompt(locale)),
      v3: ['zh','en'].every(locale => guided.guidedReadingRequestHistory([{role:'user',content:legacyPresets[locale+'-v3']}])[0].content === guided.guidedReadingPrompt(locale)),
      customV2: guided.guidedReadingRequestHistory([{role:'user',content:legacyPresets['zh-v2']}], true)[0].content === legacyPresets['zh-v2'],
    }
  })
  assert.ok(Object.values(result).every(Boolean), JSON.stringify(result))
})

for (const legacy of ['zh', 'en', 'zh-v2', 'en-v2', 'zh-v3', 'en-v3']) {
  test(`${legacy}: an existing guide follows the new rules on follow-up and regeneration without rewriting the first question`, async () => {
    await open('standalone', `&legacy=${legacy}`)
    const original = legacyPresets[legacy]
    assert.equal(await page.locator('.user-bubble').first().innerText(), '请通读这篇论文和所有图表，用易懂的例子带我逐轮读懂。')
    const composer = page.locator('.composer textarea')
    await composer.fill('继续讲，不用每次都问我问题')
    await page.locator('.send-btn').click()
    await page.waitForFunction(() => window.__test.requests.length === 1)
    const requested = await page.evaluate(() => window.__test.requests[0].args)
    assert.equal(requested.conversationId, 'legacy-guide')
    assert.equal(requested.learnUserProfile, true)
    assert.equal(requested.messages[0].content, await page.evaluate(locale => window.__test.guided.guidedReadingPrompt(locale), legacy.split('-')[0]))
    await finish()
    await page.locator('button[title="重新生成"]').last().click()
    await page.waitForFunction(() => window.__test.requests.length === 2)
    const regenerated = await page.evaluate(() => window.__test.requests[1].args)
    assert.equal(regenerated.learnUserProfile, true)
    assert.equal(regenerated.messages[0].content, requested.messages[0].content)
    await finish()
    const saved = await page.evaluate(() => window.__test.saved.at(-1).conversations.find(conv => conv.id === 'legacy-guide'))
    assert.equal(saved.nodes[0].content, original)
    await page.locator('button[title="重新生成"]').first().click()
    await page.waitForFunction(() => window.__test.requests.length === 3)
    assert.equal(await page.evaluate(() => window.__test.requests[2].args.learnUserProfile), false)
    await finish()
  })
}

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

const customGuides = {
  defaultId:'math',presets:[
    {id:'math',name:'公式推导',prompt:'逐式解释这篇论文。\n\n保留 $x_t$ 的每一步推导。'},
    {id:'experiments',name:'实验核对',prompt:'只围绕这篇论文的实验设计、图表与结论逐步讲解。'},
  ],
}
const configureGuides = () => page.evaluate(data=>localStorage.setItem('/mock-library:guides',JSON.stringify(data)),customGuides)

test('settings tab edits multiple prompts, persists the default and restores the built-in after deleting it',async()=>{
  await page.setViewportSize({width:1100,height:850})
  await open('settings')
  assert.equal(await page.locator('.qa-tab.active').innerText(),'AI 领读')
  assert.equal(await page.locator('.reading-settings textarea').getAttribute('readonly'),'')
  await page.getByRole('button',{name:'新增方案',exact:true}).click()
  await page.getByLabel('方案名称',{exact:true}).fill('公式推导')
  await page.getByLabel('提示词',{exact:true}).fill(customGuides.presets[0].prompt)
  await page.getByRole('button',{name:'新增方案',exact:true}).click()
  await page.getByLabel('方案名称',{exact:true}).fill('实验核对')
  await page.getByLabel('提示词',{exact:true}).fill(customGuides.presets[1].prompt)
  await page.getByLabel('默认方案',{exact:true}).selectOption({label:'公式推导'})
  await page.getByRole('button',{name:'保存更改',exact:true}).click()
  await page.getByText('已保存',{exact:true}).waitFor()
  await page.reload();await page.getByLabel('默认方案',{exact:true}).waitFor()
  await page.waitForFunction(()=>document.querySelector('.default-choice select')?.selectedOptions[0]?.textContent==='公式推导')
  await page.locator('.preset-row').filter({hasText:'公式推导'}).click()
  assert.equal(await page.getByLabel('提示词',{exact:true}).inputValue(),customGuides.presets[0].prompt)
  if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/guided-reading-settings.png'})}
  await page.setViewportSize({width:620,height:820})
  await page.evaluate(()=>document.documentElement.dataset.theme='midnight')
  assert.equal(await page.locator('.qa-settings').evaluate(el=>el.scrollWidth<=el.clientWidth),true)
  if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/guided-reading-settings-narrow-dark.png'})
  await page.getByRole('button',{name:'删除方案',exact:true}).click()
  assert.equal(await page.getByLabel('默认方案',{exact:true}).inputValue(),'builtin')
  await page.getByRole('button',{name:'保存更改',exact:true}).click();await page.getByText('已保存',{exact:true}).waitFor()
  const saved=await page.evaluate(()=>JSON.parse(localStorage.getItem('/mock-library:guides')))
  assert.equal(saved.defaultId,'builtin');assert.equal(saved.presets.length,1)
})

for(const mode of ['sidebar','standalone']){
  test(`${mode}: left-click uses the saved default and right-click uses another preset only once`,async()=>{
    await page.setViewportSize({width:1000,height:850});await open(mode);await configureGuides()
    const composer=page.locator('.composer textarea')
    await composer.fill('保留草稿')
    await start()
    assert.equal(await page.evaluate(()=>window.__test.requests[0].args.messages.at(-1).content),customGuides.presets[0].prompt)
    assert.equal(await composer.inputValue(),'保留草稿')
    assert.match(await page.locator('.user-bubble').innerText(),/公式推导/)
    await finish()
    await page.locator('.guided-reading-btn').click({button:'right'})
    await page.getByRole('menu',{name:'选择本次领读方案'}).waitFor()
    assert.equal(await page.evaluate(()=>window.__test.requests.length),1,'opening the menu never sends a message')
    if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/guided-reading-menu-'+mode+'.png'})
    await page.getByRole('menuitem',{name:'实验核对',exact:true}).click()
    await page.waitForFunction(()=>window.__test.requests.length===2)
    assert.equal(await page.evaluate(()=>window.__test.requests[1].args.messages.at(-1).content),customGuides.presets[1].prompt)
    await finish()
    await page.locator('.guided-reading-btn').click();await page.waitForFunction(()=>window.__test.requests.length===3)
    assert.equal(await page.evaluate(()=>window.__test.requests[2].args.messages.at(-1).content),customGuides.presets[0].prompt)
    await finish()
    const saved=await page.evaluate(()=>window.__test.saved.at(-1).conversations)
    assert(saved.some(c=>c.nodes[0].guidedReadingName==='实验核对'&&c.nodes[0].content===customGuides.presets[1].prompt))
    await page.evaluate(()=>{localStorage.setItem('/mock-library:guides',JSON.stringify({presets:[],defaultId:'builtin'}))})
    await composer.fill('继续');await composer.press('Enter');await page.waitForFunction(()=>window.__test.requests.length===4)
    assert.equal(await page.evaluate(()=>window.__test.requests[3].args.messages[0].content),customGuides.presets[0].prompt,'later configuration changes do not rewrite an existing guide')
    await finish()
  })
}

test('the preset menu supports keyboard selection, dismissal and narrow windows',async()=>{
  await page.setViewportSize({width:390,height:700});await open('standalone');await configureGuides()
  await page.locator('.guided-reading-btn').focus();await page.keyboard.press('Shift+F10')
  await page.getByRole('menu').waitFor()
  const bounds=await page.getByRole('menu').boundingBox()
  assert(bounds.x>=0&&bounds.y>=0&&bounds.x+bounds.width<=390&&bounds.y+bounds.height<=700)
  await page.keyboard.press('Escape');assert.equal(await page.getByRole('menu').count(),0)
  assert.equal(await page.evaluate(()=>window.__test.requests.length),0)
  await page.keyboard.press('Shift+F10');await page.getByRole('menu').waitFor()
  await page.keyboard.press('End');await page.keyboard.press('Enter');await page.waitForFunction(()=>window.__test.requests.length===1)
  assert.equal(await page.evaluate(()=>window.__test.requests[0].args.messages.at(-1).content),customGuides.presets[1].prompt)
  await finish()
})

test('empty prompts cannot save and save failure preserves edits and the previous persisted configuration',async()=>{
  await page.setViewportSize({width:1000,height:850});await open('settings')
  await page.getByRole('button',{name:'新增方案',exact:true}).click()
  await page.getByLabel('提示词',{exact:true}).fill('')
  assert.equal(await page.getByRole('button',{name:'保存更改',exact:true}).isDisabled(),true)
  await page.getByLabel('提示词',{exact:true}).fill('待保存的提示词')
  await page.evaluate(()=>window.__test.saveError=true)
  await page.getByRole('button',{name:'保存更改',exact:true}).click();await page.getByRole('alert').waitFor()
  assert.equal(await page.getByLabel('提示词',{exact:true}).inputValue(),'待保存的提示词')
  assert.equal(await page.evaluate(()=>localStorage.getItem('/mock-library:guides')),null)
  assert.deepEqual(errors,[])
})

test('a custom preset matching a legacy prompt is replayed verbatim',async()=>{
  await page.setViewportSize({width:1000,height:850});await open('sidebar')
  await page.evaluate(()=>{const t=window.__test;localStorage.setItem('/mock-library:guides',JSON.stringify({defaultId:'custom',presets:[{id:'custom',name:'原始节奏',prompt:t.legacyPresets.zh}]}))})
  await start();await finish()
  await page.locator('.composer textarea').fill('继续');await page.locator('.composer textarea').press('Enter')
  await page.waitForFunction(()=>window.__test.requests.length===2)
  assert.equal(await page.evaluate(()=>window.__test.requests[1].args.messages[0].content),legacyPresets.zh)
  await finish()
})

test('a library switch discards a delayed preset selection instead of starting in the new library',async()=>{
  await open('sidebar');await configureGuides();await page.evaluate(()=>window.__test.holdGuides=true)
  await page.locator('.guided-reading-btn').click()
  await page.waitForFunction(()=>!!window.__test.pendingGuideRead)
  const rootReads=await page.evaluate(()=>window.__test.rootReads)
  await page.evaluate(()=>{const t=window.__test;t.root='/another-library';t.emit('library-changed',t.root);t.pendingGuideRead.resolve(t.pendingGuideRead.config)})
  await page.waitForFunction(reads=>window.__test.rootReads>reads,rootReads)
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))))
  assert.equal(await page.evaluate(()=>window.__test.requests.length),0)
  assert.deepEqual(errors,[])
})
