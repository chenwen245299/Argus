// Real library composer, with local desktop/provider mocks and no live library.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(command,args)=>window.__test.invoke(command,args); export const convertFileSrc=p=>p; export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,cb)=>{ const set=window.__test.listeners.get(name)||new Set(); window.__test.listeners.set(name,set); set.add(cb); return ()=>set.delete(cb) };
    export const emit=async(name,payload)=>window.__test.emit(name,payload); export const emitTo=async(target,name,payload)=>window.__test.emit(name,payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'library-chat',onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}}); export class Window {static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'library-chat'}); export class WebviewWindow {static async getByLabel(){return null}}`,
}
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-context', enforce: 'pre', resolveId: s => s in mocks ? '\0mock:' + s : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null }, vue()],
    optimizeDeps: { entries: ['src/components/LibraryChat.vue'] }, server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__context')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><html><meta charset="utf-8"><body><div id="app"></div>
    <style>body{margin:0}#app,.lc-root{height:100vh}</style><script type="module">
    import {createApp} from 'vue'; import {createPinia} from 'pinia'; import {createI18n} from 'vue-i18n';
    import {addCollection} from '@iconify/vue'; import fluent from '@iconify-json/fluent/icons.json';
    import LibraryChat from '/src/components/LibraryChat.vue'; import {useAiStore} from '/src/stores/ai.ts';
    import {useSettingsStore} from '/src/stores/settings.ts'; import {zh,en} from '/src/i18n/locales.ts';
    import '/src/assets/main.css'; import '/src/assets/themes.css'; addCollection(fluent);
    const seed={id:'seed',title:'Context test',selectedPaperSlugs:[],createdAt:'2026-10-14',updatedAt:'2026-10-14',messages:[
      {id:'u1',role:'user',content:'Original research goal',createdAt:'2026-10-14'},
      {id:'a1',role:'assistant',content:'Earlier evidence and exact citations. '.repeat(80),createdAt:'2026-10-14'},
      {id:'u2',role:'user',content:'Recent question',createdAt:'2026-10-14'},
      {id:'a2',role:'assistant',content:'Recent answer',createdAt:'2026-10-14'}]};
    const test=window.__test={listeners:new Map(),calls:[],manual:[],requests:[],saved:[],autoCompact:false,
      emit(name,payload){for(const cb of test.listeners.get(name)||[]) cb({payload})},
      usage(messages){const messageTokens=messages.reduce((n,m)=>n+Math.ceil(JSON.stringify(m).length/4),0);
        return {usedTokens:messageTokens+500,contextWindow:10000,remainingTokens:9500-messageTokens,systemTokens:300,toolsTokens:200,
        messageTokens,reservedTokens:1000,thresholdTokens:8000,estimated:true,capacityEstimated:false,mediaEstimated:false,canCompact:messages.length>=3,providerId:'provider',modelId:'test-vision'}},
      prepare(args,compacted=false){const messages=compacted?[{role:'user',content:'<compacted-summary>Goal and cited evidence</compacted-summary>'},...args.messages.slice(-2)]:args.messages;
        return {messages,usage:test.usage(messages),compacted,summary:compacted?'Goal and cited evidence':null}},
      invoke(command,args){test.calls.push({command,args});
        if(command==='get_library_conversations')return Promise.resolve(JSON.parse(localStorage.getItem('context-test-saved')||'null')||[seed]);
        if(command==='get_current_library')return Promise.resolve('/mock-library');
        if(command==='get_ai_settings')return Promise.resolve(JSON.parse(JSON.stringify(test.aiSettings)));
        if(command==='list_papers')return Promise.resolve([]);
        if(command==='prepare_library_chat_context') {
          if(args.mode==='manual')return new Promise((resolve,reject)=>test.manual.push({args,resolve,reject}));
          return Promise.resolve(test.prepare(args,args.mode==='auto'&&test.autoCompact));
        }
        if(command==='chat_with_library')return new Promise((resolve,reject)=>test.requests.push({args,resolve,reject}));
        if(command==='save_library_conversation'){ const conv=JSON.parse(JSON.stringify(args.conversation));test.saved.push(conv);localStorage.setItem('context-test-saved',JSON.stringify([conv])); }
        return Promise.resolve(null);
      },
      finish(i,text='new answer'){test.requests[i].resolve(text)},
      compact(i){test.manual[i].resolve(test.prepare(test.manual[i].args,true))},
    };
    const app=createApp(LibraryChat),pinia=createPinia();app.use(pinia);app.use(createI18n({legacy:false,locale:'zh',messages:{zh,en}}));
    const ai=useAiStore(pinia);ai.loaded=true;ai.settings={default_provider_id:'provider',default_model_id:'test-vision',providers:[{
      id:'provider',name:'Example',enabled:true,kind:'custom',base_url:'https://example.test/v1',models:[{id:'test-vision',display_name:'Test vision',context_length:10000,capabilities:['vision','tool_calling']}]}]};
    test.aiSettings=JSON.parse(JSON.stringify(ai.settings));
    ai.load=async()=>{};useSettingsStore(pinia).load=async()=>{};app.mount('#app');test.mounted=true;
    </script></body></html>`))
  })
  await server.listen(); origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  const context = await browser.newContext({ viewport: { width: 1000, height: 820 } })
  await context.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
  page = await context.newPage(); page.on('pageerror', e => errors.push(e.message))
})
after(async () => { await browser?.close(); await server?.close() })
async function open(reset = true) {
  await page.setViewportSize({width:1000,height:820})
  await page.goto(`${origin}/__context`)
  if (reset) { await page.evaluate(() => localStorage.clear()); await page.reload() }
  await page.locator('.context-trigger').waitFor({ timeout: 10000 })
}
const waitManual = n => page.waitForFunction(n => window.__test.manual.length === n, n)
const waitRequests = n => page.waitForFunction(n => window.__test.requests.length === n, n)
const composer = () => page.locator('.composer textarea').last()
async function compact() {
  await page.locator('.context-trigger').click()
}
async function send(text) { await composer().fill(text); await composer().press('Enter') }

test('hover details, compact, draft preservation and persisted effective history', async () => {
  await open(); await composer().fill('unfinished draft')
  await page.locator('.context-trigger').hover()
  assert.match(await page.locator('.context-panel').innerText(), /10,000 tokens/)
  const used = (await page.locator('.context-heading span').innerText()).match(/\d+%/)[0]
  assert.equal((await page.locator('.context-trigger').innerText()).trim(), used, 'composer percentage reports usage, matching the tooltip')
  assert.match(await page.locator('.context-trigger').getAttribute('aria-label'), /已用/)
  assert.match(await page.locator('.context-panel').innerText(), /系统提示与固定文献/)
  assert.doesNotMatch(await page.locator('.context-panel').innerText(), /回答预留|自动压缩阈值|接近阈值|完整聊天记录/)
  assert.equal(await page.locator('.context-panel button').count(), 0, 'hover is a read-only usage tooltip')
  assert.equal(await page.evaluate(() => window.__test.manual.length), 0, 'hover never starts compaction')
  await compact(); await waitManual(1)
  await page.locator('.context-trigger').press('Enter')
  assert.equal(await page.evaluate(() => window.__test.manual.length), 1, 'activation while busy cannot start a second compaction')
  await page.evaluate(() => window.__test.compact(0))
  await page.waitForFunction(() => window.__test.saved.at(-1)?.contextCheckpoint)
  assert.equal(await composer().inputValue(), 'unfinished draft')
  assert.equal(await page.locator('.user-bubble').count(), 2)
  await open(false); await send('continue'); await waitRequests(1)
  const messages = await page.evaluate(() => window.__test.requests[0].args.messages)
  assert.match(messages[0].content, /compacted-summary/)
  assert.equal(messages.at(-1).content, 'continue')
  assert(!messages.some(m => m.content === 'Original research goal'))
  await page.evaluate(() => window.__test.finish(0))
})
test('summary failure and stopping never replace history, and stop pauses queued sends', async () => {
  await open(); await compact(); await waitManual(1)
  await page.evaluate(() => window.__test.manual[0].reject('summary unavailable'))
  await page.locator('.context-error').waitFor()
  assert.match(await page.locator('.context-error').innerText(), /summary unavailable/)
  assert.equal(await page.evaluate(() => window.__test.saved.some(c => c.contextCheckpoint)), false)
  await compact(); await waitManual(2); await send('queued during compression')
  await page.getByRole('button', { name: '停止', exact: true }).click()
  await page.evaluate(() => window.__test.compact(1))
  await page.waitForFunction(() => window.__test.calls.some(c => c.command === 'cancel_ai_request'))
  assert.equal(await page.evaluate(() => window.__test.requests.length), 0)
  assert.equal(await page.evaluate(() => window.__test.saved.some(c => c.contextCheckpoint)), false)
})
test('automatic preparation and an in-loop checkpoint are persisted without duplicating later turns', async () => {
  await open(); await page.evaluate(() => window.__test.autoCompact=true)
  await send('new question'); await waitRequests(1)
  const native = [{role:'user',content:'<compacted-summary>Loop evidence</compacted-summary>'},
    {role:'user',content:'new question'},{role:'assistant',content:'new answer'}]
  await page.evaluate(messages => { const t=window.__test,r=t.requests[0];
    t.emit(r.args.eventName+'-context-checkpoint',{messages}); t.finish(0); }, native)
  await page.waitForFunction(() => window.__test.saved.at(-1)?.contextCheckpoint?.throughMessageId === window.__test.saved.at(-1)?.messages.at(-1)?.id)
  await page.evaluate(() => window.__test.autoCompact=false)
  await send('next'); await waitRequests(2)
  const messages = await page.evaluate(() => window.__test.requests[1].args.messages)
  assert.deepEqual(messages.slice(0,3),native)
  assert.equal(messages.length,4)
  await page.evaluate(() => window.__test.finish(1))
})
test('late compaction after a library switch cannot save into the new library', async () => {
  await open(); await compact(); await waitManual(1)
  await page.evaluate(() => window.__test.emit('library-changed', {}))
  await page.evaluate(() => window.__test.compact(0))
  await page.waitForFunction(() => window.__test.calls.filter(c=>c.command==='get_library_conversations').length>=2)
  assert.equal(await page.evaluate(() => window.__test.saved.some(c => c.contextCheckpoint)),false)
})
test('meter fits narrow and dark views, and remains keyboard accessible', async () => {
  await open()
  if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.locator('.composer').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/context-composer-light.png'})}
  await page.setViewportSize({width:560,height:720})
  await page.evaluate(() => document.documentElement.dataset.theme='midnight')
  await page.locator('.context-trigger').focus()
  await page.locator('.context-panel').waitFor()
  const box=await page.locator('.context-panel').boundingBox()
  assert(box.x>=0&&box.y>=0&&box.x+box.width<=560)
  const trigger=await page.locator('.context-trigger').boundingBox()
  const sendBox=await page.locator('.send-btn').last().boundingBox()
  assert(trigger.x>=0&&sendBox.x+sendBox.width<=560)
  const card=await page.locator('.composer').last().boundingBox()
  assert(trigger.y>=card.y&&trigger.y+trigger.height<=card.y+card.height, 'context meter stays inside the input card')
  assert(Math.abs(trigger.y+trigger.height/2-sendBox.y-sendBox.height/2)<=1, 'context meter shares the send row')
  const tool=await page.locator('.footer-left .toolbar-btn').first().boundingBox()
  assert.equal(sendBox.width,tool.width)
  assert.equal(sendBox.height,tool.height)
  if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/context-meter-dark.png'})}
  await page.keyboard.press('Escape')
  assert.equal(await page.locator('.context-panel').count(),0)
  assert.equal(await page.evaluate(() => window.__test.manual.length),0)
  await page.locator('.context-trigger').press('Enter'); await waitManual(1)
  await page.evaluate(() => window.__test.compact(0))
  await page.waitForFunction(() => window.__test.saved.at(-1)?.contextCheckpoint)
  assert.deepEqual(errors,[])
})

test('composer statistics use cumulative snapshots and survive regeneration and reload', async () => {
  await open(); await send('Statistics question'); await waitRequests(1)
  await page.evaluate(() => { const t=window.__test,r=t.requests[0];
    const sample={input_tokens:1000,output_tokens:100,total_tokens:1100,cache_hit_tokens:800,cache_reported:true,model_calls:2,model_duration_ms:1000};
    t.emit(r.args.eventName+'-run-usage',sample);t.emit(r.args.eventName+'-run-usage',sample);t.finish(0)
  })
  await page.waitForFunction(()=>window.__test.saved.at(-1)?.usageRuns)
  assert.match(await page.locator('.chat-run-stats').innerText(),/3 轮 2 步/)
  assert.match(await page.locator('.chat-run-stats').innerText(),/1.1k tok/)
  assert.match(await page.locator('.chat-run-stats').innerText(),/缓存命中 80%/)
  await open(false)
  assert.match(await page.locator('.chat-run-stats').innerText(),/1.1k tok/)
  await page.locator('.assistant-action-row').last().hover()
  await page.locator('button[title="重新生成"]').last().click();await waitRequests(1)
  await page.evaluate(()=>{const t=window.__test,r=t.requests[0];
    t.emit(r.args.eventName+'-run-usage',{input_tokens:1000,output_tokens:100,total_tokens:1100,cache_hit_tokens:800,cache_reported:true,model_calls:2,model_duration_ms:1000});t.finish(0)
  })
  await page.waitForFunction(()=>!document.querySelector('.stop-btn'))
  assert.match(await page.locator('.chat-run-stats').innerText(),/3 轮 4 步/)
  assert.match(await page.locator('.chat-run-stats').innerText(),/2.2k tok/)
  assert.equal(await page.locator('.composer-footer .chat-run-stats').count(),1)
  const stats=await page.locator('.chat-run-stats').boundingBox()
  const tool=await page.locator('.footer-left .toolbar-btn').first().boundingBox()
  const sendBox=await page.locator('.send-btn').last().boundingBox()
  assert(Math.abs(stats.y+stats.height/2-tool.y-tool.height/2)<=1, 'statistics share the tools row')
  assert(Math.abs(stats.y+stats.height/2-sendBox.y-sendBox.height/2)<=1, 'statistics share the send row')
  assert.equal(await page.locator('.run-stats-content').evaluate(el=>el.scrollHeight<=20),true, 'statistics occupy one line')
  if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/context-stats-library.png'})
})

test('stopping keeps final accounting listeners until the cancelled request settles',async()=>{
  await open();await send('Stop with partial usage');await waitRequests(1)
  await page.evaluate(()=>{const t=window.__test,name=t.requests[0].args.eventName;
    t.emit(name+'-run-usage',{input_tokens:100,output_tokens:20,total_tokens:120,model_calls:1})
  })
  await page.getByRole('button',{name:'停止',exact:true}).click()
  await page.evaluate(()=>{const t=window.__test,name=t.requests[0].args.eventName;
    t.emit(name+'-run-usage',{input_tokens:200,output_tokens:30,total_tokens:230,model_calls:2});t.finish(0)
  })
  await page.waitForFunction(()=>window.__test.saved.at(-1)?.usageRuns&&Object.values(window.__test.saved.at(-1).usageRuns).some(run=>run.totalTokens===230))
  assert.match(await page.locator('.chat-run-stats').innerText(),/230 tok/)
})
