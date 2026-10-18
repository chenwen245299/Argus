// Real settings UI with local desktop I/O simulated; no user data or API calls.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, context, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (command, args) => window.__test.invoke(command, args); export const convertFileSrc = path => path; export const isTauri = () => false;`,
  '@tauri-apps/api/event': `export const listen = async (name, cb) => { const set = window.__test.listeners.get(name) || new Set(); window.__test.listeners.set(name,set); set.add(cb); return () => set.delete(cb); }; export const emit = async (name,payload) => window.__test.emit(name,payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow = () => ({ label:'main', onResized:async()=>()=>{}, onFocusChanged:async()=>()=>{} });`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow = () => ({label:'main'}); export class WebviewWindow { static async getByLabel() { return null; } }`,
}
before(async () => {
  server = await createServer({ configFile:false, root:new URL('../',import.meta.url).pathname, appType:'custom',
    plugins:[{ name:'mock-desktop', enforce:'pre', resolveId:source => source in mocks ? '\0mock:'+source : null,
      load:id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null },vue()],
    optimizeDeps:{entries:['src/components/settings/QaSettings.vue']},
    server:{host:'127.0.0.1',port:0}, logLevel:'error' })
  server.middlewares.use(async (req,res,next) => {
    if (!req.url.startsWith('/__memory-test')) return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html,body,#app{margin:0;width:100%;height:100%}</style><script type="module">
      import {createApp,h} from 'vue'; import {createPinia} from 'pinia'; import {createI18n} from 'vue-i18n';
      import QaSettings from '/src/components/settings/QaSettings.vue'; import {zh,en} from '/src/i18n/locales.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      const initial = {enabled:true,autoLearn:true,entries:[],revision:0,lastCheckedAt:null,lastError:null,updating:false};
      const test=window.__test={listeners:new Map(),calls:[],fail:false,readFail:false,
        profile:JSON.parse(localStorage.getItem('memory')||'null')||initial,
        emit(name,payload){ for(const cb of test.listeners.get(name)||[])cb({payload}); },
        publish(patch){Object.assign(test.profile,patch);test.profile.revision++;localStorage.setItem('memory',JSON.stringify(test.profile));test.emit('user-memory-changed');},
        entry(id,content,level='new'){return {id,key:'topic.'+id,category:'knowledge',content,knowledgeLevel:level,evidence:content,source:'paper',createdAt:'2026-10-18T12:00:00Z',updatedAt:'2026-10-18T12:00:00Z',locked:false};},
        async invoke(command,args){test.calls.push({command,args});
          if(command==='get_user_memory'){
            if(test.readFail)throw new Error('read failed');
            const value=structuredClone(test.profile);
            if(test.holdRead)return new Promise(resolve=>test.heldRead={resolve,value});
            return value;
          }
          if(test.fail)throw new Error('write failed');
          if(command==='configure_user_memory')test.publish({enabled:args.enabled,autoLearn:args.autoLearn});
          if(command==='edit_user_memory'){
            const edit=args.edit,entry=test.profile.entries.find(e=>e.id===edit.id);
            if(entry.updatedAt!==edit.expectedUpdatedAt)throw new Error('stale edit');
            Object.assign(entry,edit,{evidence:'',source:'manual',updatedAt:'2026-10-19T12:00:00Z'});test.publish({});
          }
          if(command==='delete_user_memory')test.publish({entries:args.id?test.profile.entries.filter(e=>e.id!==args.id):[]});
          return structuredClone(test.profile);
        }};
      const query=new URLSearchParams(location.search);document.documentElement.dataset.theme=query.get('theme')||'light';
      createApp({render:()=>h(QaSettings,{initialTab:'memory'})}).use(createPinia())
        .use(createI18n({legacy:false,locale:query.get('locale')||'zh',messages:{zh,en}})).mount('#app');
      test.mounted=true;
      </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true})
  context=await browser.newContext({viewport:{width:1024,height:820}})
  await context.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
  page=await context.newPage();page.on('pageerror',e=>errors.push(e.message))
})
after(async()=>{await browser?.close();await server?.close()})
async function open(extra='') {
  await page.goto(origin+'/__memory-test'+extra)
  await page.waitForFunction(()=>window.__test?.mounted)
  await page.locator('.memory-toggle input').waitFor()
  await page.evaluate(()=>{localStorage.clear();window.__test.publish({enabled:true,autoLearn:true,entries:[],lastError:null,updating:false})})
  await page.waitForFunction(()=>document.querySelector('.memory-count')?.textContent==='0')
}
async function seed() {
  await page.evaluate(()=>window.__test.publish({entries:[window.__test.entry('algebra','我学过线性代数','familiar'),window.__test.entry('kl','我没学过 KL 散度')]}))
  await page.locator('.memory-entry').nth(1).waitFor()
}
test('memory tab defaults to automatic learning without any manual generation action',async()=>{
  await open()
  assert.equal(await page.locator('.qa-tab.active').textContent(),'用户记忆')
  assert.ok(await page.locator('.memory-toggle input').isChecked())
  assert.match(await page.locator('.memory-empty').textContent(),/正常聊天/)
  assert.equal(await page.getByRole('button',{name:/生成|提取|Generate/}).count(),0)
  assert.ok((await page.evaluate(()=>window.__test.calls)).every(c=>c.command==='get_user_memory'))
})
test('automatic background changes refresh the visible profile and show evidence',async()=>{
  await seed()
  assert.equal(await page.locator('.memory-entry').count(),2)
  assert.deepEqual(await page.locator('.memory-level').allTextContents(),['熟悉','刚接触'])
  await page.locator('.memory-evidence summary').nth(1).click()
  assert.match(await page.locator('.memory-evidence blockquote').nth(1).textContent(),/没学过 KL/)
  await page.evaluate(()=>window.__test.publish({updating:true}))
  await page.waitForFunction(()=>document.querySelector('.memory-status')?.textContent.includes('正在根据'))
  await page.evaluate(()=>{const entry=window.__test.profile.entries[1];entry.content='正在学习 KL 散度';entry.knowledgeLevel='learning';window.__test.publish({updating:false})})
  await page.waitForFunction(()=>document.querySelectorAll('.memory-level')[1]?.textContent==='正在学习')
})
test('turning memory off persists across reopening and keeps existing records',async()=>{
  await page.locator('.memory-toggle input').uncheck()
  await page.waitForFunction(()=>document.querySelector('.memory-status')?.textContent.startsWith('已关闭'))
  assert.equal(await page.locator('.memory-entry').count(),2)
  await page.reload();await page.locator('.memory-toggle input').waitFor()
  assert.equal(await page.locator('.memory-toggle input').isChecked(),false)
  assert.deepEqual(await page.evaluate(()=>({enabled:window.__test.profile.enabled,autoLearn:window.__test.profile.autoLearn})),{enabled:false,autoLearn:false})
})
test('corrections can protect a fact and preserve the knowledge category',async()=>{
  await page.locator('.memory-entry').nth(1).getByRole('button',{name:'纠正'}).click()
  await page.locator('.memory-field textarea').fill('我已系统学过 KL 散度')
  await page.locator('select').nth(1).selectOption('familiar')
  await page.locator('.memory-lock input').check()
  await page.getByRole('button',{name:'保存',exact:true}).click()
  await page.locator('.memory-field textarea').waitFor({state:'hidden'})
  assert.match(await page.locator('.memory-entry').nth(1).textContent(),/已保护/)
  assert.match(await page.locator('.memory-entry').nth(1).textContent(),/已手动纠正/)
  assert.equal(await page.locator('.memory-entry').nth(1).locator('details').count(),0)
})
test('failed writes retain the edit, while a background change produces a stale-edit conflict',async()=>{
  await page.locator('.memory-entry').first().getByRole('button',{name:'纠正'}).click()
  await page.locator('textarea').fill('我也学过概率论')
  await page.evaluate(()=>window.__test.fail=true)
  await page.getByRole('button',{name:'保存',exact:true}).click()
  await page.locator('[role=alert]').waitFor()
  assert.equal(await page.locator('textarea').inputValue(),'我也学过概率论')
  await page.evaluate(()=>{window.__test.fail=false;window.__test.profile.entries[0].updatedAt='2026-10-20T00:00:00Z';window.__test.publish({})})
  await page.getByRole('button',{name:'保存',exact:true}).click()
  assert.equal(await page.locator('textarea').inputValue(),'我也学过概率论')
  await page.locator('textarea').press('Escape')
  await page.locator('textarea').waitFor({state:'hidden'})
})
test('single delete and clear require explicit confirmation and do not auto-trigger extraction',async()=>{
  const entry=page.locator('.memory-entry').first()
  await entry.getByRole('button',{name:'删除',exact:true}).click()
  assert.equal(await page.locator('.memory-entry').count(),2)
  await entry.locator('.memory-confirm').getByRole('button',{name:'取消'}).click()
  assert.equal(await page.locator('.memory-entry').count(),2)
  await entry.getByRole('button',{name:'删除',exact:true}).click()
  await entry.locator('.memory-confirm').getByRole('button',{name:'删除'}).click()
  await page.waitForFunction(()=>document.querySelectorAll('.memory-entry').length===1)
  await page.getByRole('button',{name:'清空画像',exact:true}).click()
  await page.getByRole('button',{name:'确认清空'}).click()
  await page.locator('.memory-empty').waitFor()
  assert.ok((await page.evaluate(()=>window.__test.calls)).every(c=>!c.command.includes('extract')))
})
test('narrow dark and English layouts remain usable without horizontal overflow',async()=>{
  await open('?theme=dark')
  await seed()
  await page.setViewportSize({width:520,height:800})
  await page.locator('.memory-entry').first().getByRole('button',{name:'纠正'}).click()
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth))
  await mkdir('/private/tmp/argus-memory',{recursive:true})
  await page.screenshot({path:'/private/tmp/argus-memory/settings-narrow-dark.png'})
  await page.setViewportSize({width:1024,height:820})
  await open('?locale=en')
  await seed()
  assert.equal(await page.locator('.qa-tab.active').textContent(),'User memory')
  assert.match(await page.locator('.memory-toggle').textContent(),/Remember my knowledge/)
  await page.screenshot({path:'/private/tmp/argus-memory/settings-english.png'})
  assert.deepEqual(errors,[])
})
