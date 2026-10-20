// Real settings UI and paper composers in separate browser pages; localhost mocks only.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, context, origin, settingsPage, popup, sidebar
const pages = [], errors = [], requests = [], saved = []
const model = (id, name = id) => ({ id, display_name: name, enabled: true, capabilities: ['tool_calling'], context_length: 100000 })
let config = { default_provider_id: 'p', default_model_id: 'original', providers: [{
  id: 'p', name: 'Example', kind: 'custom', base_url: 'https://example.test/v1', enabled: true,
  has_key: true, server_tools: {}, models: [model('original', 'Original model')],
}] }
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__desktopInvoke(name,args);export const convertFileSrc=p=>p;export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,cb)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(cb);return()=>set.delete(cb)};
    export const emit=async()=>{};export const emitTo=async()=>{};`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:window.__test.mode==='standalone'?'paper-ai-cGFwZXI':'main',onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}});export class Window{static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'main'});export class WebviewWindow{static async getByLabel(){return null}}`,
}
const emit = (page, name, payload) => page.evaluate(({name,payload}) => window.__test.emit(name,payload), {name,payload})
async function broadcast(name, payload) { await Promise.all(pages.map(page => emit(page,name,payload))) }
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-model-sync', enforce: 'pre', resolveId: s => s in mocks ? '\0mock:' + s : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null }, vue()],
    optimizeDeps: { entries: ['src/components/tabs/AiTab.vue', 'src/components/settings/AiSettings.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req,res,next) => {
    if (!req.url.startsWith('/__model-sync')) return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html,body,#app,.test-panel{margin:0;width:100%;height:100%}</style><script type="module">
      import {createApp,h} from 'vue';import {createPinia} from 'pinia';import {createI18n} from 'vue-i18n';
      import {addCollection} from '@iconify/vue';import fluent from '@iconify-json/fluent/icons.json';
      import AiTab from '/src/components/tabs/AiTab.vue';import AiSettings from '/src/components/settings/AiSettings.vue';
      import {useAiStore} from '/src/stores/ai.ts';import {useSettingsStore} from '/src/stores/settings.ts';import {zh,en} from '/src/i18n/locales.ts';
      import '/src/assets/main.css';import '/src/assets/themes.css';addCollection(fluent);
      const mode=new URLSearchParams(location.search).get('mode');
      const t=window.__test={mode,listeners:new Map(),emit(name,payload){for(const cb of t.listeners.get(name)||[])cb({payload})}};
      const pinia=createPinia();const app=createApp({setup(){return()=>h('div',{class:'test-panel'},[mode==='settings'?h(AiSettings):h(AiTab,{slug:'paper',standalone:mode==='standalone'})])}});
      app.use(pinia);app.use(createI18n({legacy:false,locale:'zh',messages:{zh,en}}));
      t.ai=useAiStore(pinia);useSettingsStore(pinia).load=async()=>{};
      app.mount('#app');t.mounted=true;
      </script>`))
  })
  await server.listen();origin=`http://127.0.0.1:${server.httpServer.address().port}`
  browser=await chromium.launch({channel:'chrome',headless:true})
  context=await browser.newContext({viewport:{width:1000,height:850}})
  await context.route('**/*',r=>r.request().url().startsWith(origin)?r.continue():r.abort())
  await context.exposeBinding('__desktopInvoke',async ({page},name,args) => {
    if(name==='get_current_library')return '/mock-library'
    if(name==='get_ai_settings'){assert.equal(args.expectedRoot,'/mock-library');return structuredClone(config)}
    if(name==='fetch_provider_balances')return []
    if(name==='get_paper_ai_conversations'||name==='get_paper_chat'||name==='get_chat_history')return []
    if(name==='get_paper_meta')return {title:'Example paper',abstract:'A complete abstract'}
    if(name==='update_ai_provider'){
      Object.assign(config.providers.find(p=>p.id===args.provider.id),args.provider)
      await broadcast('ai-settings-changed',{root:'/mock-library'});return null
    }
    if(name==='save_provider_models'){
      config.providers.find(p=>p.id===args.id).models=structuredClone(args.models)
      await broadcast('ai-settings-changed',{root:'/mock-library'});return null
    }
    if(name==='chat_with_library')return new Promise((resolve,reject)=>requests.push({page,args,resolve,reject}))
    if(name==='save_paper_ai_conversations'){saved.push(structuredClone(args));return null}
    return null
  })
  for(const mode of ['settings','standalone','panel']){
    const page=await context.newPage();pages.push(page);page.on('pageerror',e=>errors.push(e.message))
    await page.goto(origin+'/__model-sync?mode='+mode)
    await page.waitForFunction(()=>window.__test?.mounted&&window.__test.ai.loaded)
    if(mode==='settings')settingsPage=page;else if(mode==='standalone')popup=page;else sidebar=page
  }
})
after(async()=>{requests.forEach(r=>r.resolve('Completed'));await browser?.close();await server?.close()})
async function addFromSettings(id,name){
  await settingsPage.getByRole('button',{name:'手动添加',exact:true}).click()
  await settingsPage.getByPlaceholder('gpt-4.1-mini',{exact:true}).fill(id)
  await settingsPage.getByPlaceholder('显示名称',{exact:true}).fill(name)
  await settingsPage.locator('.model-form-actions .btn-primary').click()
  await popup.waitForFunction(id=>window.__test.ai.chatModels.some(m=>m.modelId===id),id)
}

test('adding a model in settings immediately updates open standalone and sidebar menus while retaining drafts and selection',async()=>{
  await popup.locator('.composer-input').fill('Unsent standalone draft')
  await sidebar.locator('.composer-input').fill('Unsent sidebar draft')
  await popup.locator('.model-trigger').click()
  await sidebar.locator('.model-trigger').click()
  await addFromSettings('added','Added in settings')
  for(const page of [popup,sidebar]){
    await page.locator('.model-row').filter({hasText:'Added in settings'}).waitFor()
    assert.match(await page.locator('.model-trigger').innerText(),/Original model/)
  }
  assert.equal(await popup.locator('.composer-input').inputValue(),'Unsent standalone draft')
  assert.equal(await sidebar.locator('.composer-input').inputValue(),'Unsent sidebar draft')
  await popup.locator('.model-row').filter({hasText:'Added in settings'}).click()
  await popup.locator('.composer-input').fill('Use the newly added model')
  await popup.locator('.composer-input').press('Enter')
  await popup.waitForFunction(()=>document.querySelector('.stop-btn'))
  while(!requests.length)await new Promise(resolve=>setTimeout(resolve,10))
  assert.equal(requests[0].args.modelId,'added')
  await emit(popup,requests[0].args.eventName,{delta:'Partial answer from added model',done:false})
  await popup.getByText('Partial answer from added model',{exact:true}).waitFor()
})

test('another settings edit preserves an in-flight answer, its captured model and the next draft',async()=>{
  await popup.locator('.composer-input').fill('Next unsent question')
  await addFromSettings('third','Third model')
  assert.match(await popup.locator('.model-trigger').innerText(),/Added in settings/)
  assert.equal(await popup.locator('.composer-input').inputValue(),'Next unsent question')
  assert.equal(requests.length,1)
  await popup.getByText('Partial answer from added model',{exact:true}).waitFor()
  await emit(popup,requests[0].args.eventName,{delta:' — completed.',done:false})
  requests[0].resolve('Partial answer from added model — completed.')
  await popup.locator('.stop-btn').waitFor({state:'hidden'})
  assert.equal(await popup.locator('.composer-input').inputValue(),'Next unsent question')
  assert(saved.some(s=>s.conversations.some(c=>c.nodes.some(n=>n.answers?.some(a=>a.modelId==='added'&&a.content.includes('completed.'))))))
})

test('removing the selected model updates the open menu and falls back without losing the draft',async()=>{
  await settingsPage.evaluate(()=>window.__test.ai.saveModels('p',window.__test.ai.settings.providers[0].models.filter(m=>m.id!=='added')))
  await popup.waitForFunction(()=>!window.__test.ai.chatModels.some(m=>m.modelId==='added'))
  await popup.waitForFunction(()=>document.querySelector('.model-trigger')?.textContent.includes('Original model'))
  assert.equal(await popup.locator('.composer-input').inputValue(),'Next unsent question')
  await popup.locator('.model-trigger').click()
  assert.equal(await popup.locator('.model-row').filter({hasText:'Added in settings'}).count(),0)
  assert.equal(errors.length,0,errors.join('\n'))
})
