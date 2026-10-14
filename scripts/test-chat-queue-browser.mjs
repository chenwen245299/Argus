// Real Vue composers with mocked desktop/provider I/O. No external requests.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
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
    if (!req.url.startsWith('/__queue-test')) return next()
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
      import { useAiStore } from '/src/stores/ai.ts';
      import { useSettingsStore } from '/src/stores/settings.ts';
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
      const app = createApp({setup(){return () => mode === 'library' ? h(LibraryChat)
        : h('div',{class:'test-panel'},[h(AiTab,{slug:slug.value,standalone:mode === 'standalone'})]);}});
      const pinia=createPinia(); app.use(pinia);
      app.use(createI18n({legacy:false,locale:'zh',messages:{zh,en}}));
      const ai=useAiStore(pinia); ai.loaded=true;
      ai.settings={default_provider_id:'provider',default_model_id:'test-vision',providers:[{
        id:'provider',name:'Example',enabled:true,kind:'custom',base_url:'https://example.test/v1',
        models:[{id:'test-vision',display_name:'Test vision',capabilities:['vision','tool_calling']}],
      }]};
      ai.load=async()=>{};
      useSettingsStore(pinia).load=async()=>{};
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
  await page.goto(`${origin}/__queue-test?mode=${mode}`)
  await page.waitForFunction(() => window.__test?.mounted, null, { timeout: 10000 }).catch(async error => {
    console.error(await page.evaluate(() => ({ html: document.body.innerHTML.slice(0, 200), calls: window.__test?.calls, errors: window.__test?.errors })))
    throw error
  })
  await page.locator('.composer textarea').waitFor()
  await page.waitForFunction(() => window.__test.calls.some(c => c.command === 'list_papers' || c.command === 'get_paper_meta'))
}
const composer = () => page.locator('.composer textarea').last()
async function send(text, key = 'Enter') { await composer().fill(text); await composer().press(key) }
const count = n => page.waitForFunction(n => window.__test.requests.length === n, n)
const finish = (index, text) => page.evaluate(({index,text}) => window.__test.finish(index,text), {index,text})
const history = index => page.evaluate(index => window.__test.requests[index].args.messages, index)

for (const mode of ['sidebar', 'standalone', 'library']) {
  test(`${mode}: normal sends queue, edit preserves draft and FIFO, delete does not send`, async () => {
    await open(mode); await send('first'); await count(1)
    await send('second'); await send('remove me'); await send('third')
    assert.equal(await page.locator('.queue-item').count(), 3)
    await composer().fill('unfinished draft')
    const second = page.locator('.queue-item').first()
    await second.getByRole('button', { name: '编辑', exact: true }).click()
    await second.locator('textarea').fill('edited second')
    await finish(0, 'partial first answer')
    assert.equal(await page.evaluate(() => window.__test.requests.length), 1, 'editing holds the queue on completion')
    await second.getByRole('button', { name: '保存', exact: true }).click(); await count(2)
    await page.locator('.queue-item').filter({ hasText: 'remove me' }).getByRole('button', { name: '删除' }).click()
    await finish(1, 'second answer'); await count(3)
    assert.equal((await history(1)).at(-1).content, 'edited second')
    assert.equal((await history(2)).at(-1).content, 'third')
    assert.equal(await composer().inputValue(), 'unfinished draft')
    assert.equal(await page.locator('.queue-item').count(), 0)
    await finish(2)
  })

  test(`${mode}: Cmd+Enter and Steer cancel the exact request and preserve partial text and tool results`, async () => {
    await open(mode); await send('research'); await count(1)
    await page.evaluate(() => {
      const r=window.__test.requests[0];
      window.__test.emit(r.args.eventName,{delta:'already written',done:false});
      window.__test.emit(r.args.eventName+'-agent',{phase:'tool',tool:'get_paper',arguments:{slug:'paper'}});
      window.__test.emit(r.args.eventName+'-agent',{phase:'result',tool:'get_paper',ok:true,preview:'cached paper result'});
    })
    await send('later'); await send('change direction','Meta+Enter'); await count(2)
    const messages = await history(1)
    assert.ok(messages.some(m => m.role === 'assistant' && m.content === 'already written'))
    assert.ok(messages.some(m => m.role === 'tool' && m.content === 'cached paper result'))
    assert.equal(messages.at(-1).content, 'change direction')
    assert.equal(await page.locator('.queue-item').count(), 1)
    await page.locator('.queue-item').getByRole('button',{name:'引导',exact:true}).click(); await count(3)
    assert.equal((await history(2)).at(-1).content,'later')
    const cancellations = await page.evaluate(() => window.__test.calls.filter(c => c.command==='cancel_ai_request').map(c => c.args.requestId))
    assert.deepEqual(cancellations, await page.evaluate(() => window.__test.requests.slice(0,2).map(r=>r.args.requestId)))
    assert.equal(await page.locator('.queue-item').count(),0)
    await finish(2)
  })

  test(`${mode}: stop pauses queued attachments, Ctrl+Enter steers, IME and Shift+Enter do not send`, async () => {
    await open(mode); await send('first'); await count(1)
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+afoIAAAAASUVORK5CYII=', 'base64')
    await page.locator('input[type=file]').first().setInputFiles({name:'diagram.png',mimeType:'image/png',buffer:png})
    await page.locator('.composer .attachment-chip').waitFor()
    await send('with attachment')
    assert.match(await page.locator('.queue-item').textContent(), /diagram.png/)
    await page.locator('.stop-btn').click()
    await page.getByRole('button',{name:'继续发送',exact:true}).waitFor()
    assert.equal(await page.evaluate(()=>window.__test.requests.length),1)
    await page.getByRole('button',{name:'继续发送',exact:true}).click(); await count(2)
    const parts = (await history(1)).at(-1).content
    assert.ok(parts.some(p=>p.type==='image_url'))
    await send('draft','Shift+Enter')
    assert.equal(await page.evaluate(()=>window.__test.requests.length),2)
    await composer().evaluate(el=>el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',isComposing:true,bubbles:true,cancelable:true})))
    assert.equal(await page.evaluate(()=>window.__test.requests.length),2)
    await send('urgent','Control+Enter'); await count(3)
    await finish(2)
    assert.deepEqual(errors,[])
  })
}

test('paper switching keeps queued messages with their original paper and leaves the new draft intact', async () => {
  await open('sidebar'); await send('paper one'); await count(1); await send('queued paper one')
  await page.evaluate(()=>window.__test.switchPaper('other'))
  await page.waitForFunction(()=>window.__test.calls.some(c=>c.command==='get_paper_meta'&&c.args.slug==='other'))
  await composer().fill('other draft'); await finish(0); await count(2)
  assert.equal(await page.evaluate(()=>window.__test.requests[1].args.paperSlug),'paper')
  assert.equal(await composer().inputValue(),'other draft')
  assert.equal(await page.locator('.queue-item').count(),0)
  await finish(1)
})

test('switching libraries clears queued messages and refuses late completion', async () => {
  for (const mode of ['sidebar','library']) {
    await open(mode); await send('old root'); await count(1); await send('must not send')
    await page.evaluate(()=>window.__test.emit('library-changed','/new-library'))
    await page.waitForFunction(()=>window.__test.calls.some(c=>c.command==='cancel_ai_request'))
    assert.equal(await page.locator('.queue-item').count(),0)
    assert.equal(await page.evaluate(()=>window.__test.requests.length),1)
  }
  assert.deepEqual(errors,[])
})

test('steering refuses only approvals belonging to the stopped conversation', async () => {
  for (const mode of ['sidebar','library']) {
    await open(mode); await send('first conversation'); await count(1)
    await page.locator('.composer .toolbar-btn[title="新建对话"]').click()
    await send('second conversation'); await count(2)
    await page.evaluate(() => {
      for (let i=0;i<2;i++) window.__test.emit(window.__test.requests[i].args.eventName+'-confirm',{
        requestId:'approval-'+i,preview:{tool:'create_paper_note',slug:'paper',paperTitle:'Paper',noteTitle:'Review',content:'Body',duplicateTitle:false},
      });
    })
    await page.locator('.write-confirm').waitFor()
    await send('steer second','Meta+Enter'); await count(3)
    const decisions=await page.evaluate(()=>window.__test.calls.filter(c=>c.command==='resolve_agent_write').map(c=>c.args))
    assert.deepEqual(decisions,[{requestId:'approval-1',approved:false}])
    assert.equal(await page.locator('.write-confirm').count(),1,'the other conversation still requires its own approval')
    await finish(0); await finish(2)
  }
})

test('queue controls fit a narrow sidebar, and attachment-only messages can be queued', async () => {
  await open('sidebar')
  await page.locator('.test-panel').evaluate(el=>el.style.width='350px')
  await send('first'); await count(1)
  await page.locator('input[type=file]').first().setInputFiles({name:'paper.pdf',mimeType:'application/pdf',buffer:Buffer.from('%PDF-1.4 mock')})
  await page.locator('.composer .attachment-chip').waitFor()
  await composer().press('Enter')
  assert.match(await page.locator('.queue-item').textContent(),/paper.pdf/)
  await page.locator('.queue-item').getByRole('button',{name:'编辑',exact:true}).click()
  assert.ok(await page.locator('.queue-edit').evaluate(el=>document.activeElement===el))
  await page.locator('.queue-edit').press('Escape')
  const fits=await page.locator('.composer').evaluate(el=>{
    const right=el.getBoundingClientRect().right;
    return [...el.querySelectorAll('button,textarea')].every(child=>child.getBoundingClientRect().right<=right+1);
  })
  assert.ok(fits,'queue and toolbar controls stay within the sidebar')
  await page.screenshot({path:'/tmp/argus-chat-queue-sidebar.png'})
  await page.locator('.stop-btn').click()
  assert.deepEqual(errors,[])
})
