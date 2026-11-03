// Real import queue, store and native-dialog helper; isolated local library/IPC.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args);`,
  '@tauri-apps/api/event': `export const listen=async()=>()=>{};`,
}
const storeMocks = {
  './library': 'useLibraryStore', './reader': 'useReaderStore', './selection': 'useSelectionStore',
  './collections': 'useCollectionsStore', './ranks': 'useRanksStore', './settings': 'useSettingsStore',
}
before(async () => {
  server = await createServer({ configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',
    plugins:[{name:'mock-import-io',enforce:'pre',resolveId(source,importer) {
      if(source in mocks)return '\0import-mock:'+source
      if(importer?.endsWith('/src/stores/import.ts')&&source in storeMocks)return '\0import-store:'+source
    },load(id) {
      if(id.startsWith('\0import-mock:'))return mocks[id.slice(13)]
      if(id.startsWith('\0import-store:')) { const name=storeMocks[id.slice(14)];return `export const ${name}=()=>window.__test.${name};` }
    }},vue()],optimizeDeps:{entries:['src/components/ImportQueue.vue']},
    server:{host:'127.0.0.1',port:0},logLevel:'error'})
  server.middlewares.use(async(req,res,next)=>{
    if(!req.url.startsWith('/__imports'))return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>body{margin:0;padding:16px}#app{display:flex;justify-content:flex-start;gap:10px}button{font:inherit}</style>
      <script type="module">
      import{createApp,h}from'vue';import{createPinia}from'pinia';import{useImportStore}from'/src/stores/import.ts';
      import ImportQueue from'/src/components/ImportQueue.vue';import{ i18n }from'/src/i18n/index.ts';
      import'/src/assets/main.css';import'/src/assets/themes.css';
      const t=window.__test={calls:[],questions:[],disk:new Set(),failCleanup:false,
        useLibraryStore:{currentPath:'/mock-library',refresh:async()=>{}},useReaderStore:{replacePaperSlug(){}},
        useSelectionStore:{selectedSlug:'',selectPaper(slug){this.selectedSlug=slug}},
        useCollectionsStore:{canReceivePapers:()=>true,load:async()=>{}},useRanksStore:{},
        useSettingsStore:{loaded:true,easyscholarConfigured:false},
        invoke(name,args){t.calls.push({name,args});
          if(name==='plugin:dialog|message')return new Promise((resolve,reject)=>t.questions.push({args,resolve,reject}));
          if(name==='import_ebook'||name==='import_pdf'){
            if(args.sourcePath.includes('broken'))return Promise.reject('Damaged archive: missing OPF');
            const slug=args.sourcePath.split('/').pop();t.disk.add(slug);return Promise.resolve(slug);
          }
          if(name==='extract_import_metadata'){
            if(args.slug.includes('uncertain'))return Promise.reject('Title not established after ten pages');
            return Promise.resolve({meta:{id:args.slug,title:args.slug},isPaper:false,isBook:true});
          }
          if(name==='find_duplicate_paper')return Promise.resolve(args.slug.includes('duplicate')?{existingSlug:'original',title:'以利为利：财政关系与地方政府行为'}:null);
          if(name==='delete_paper'){if(t.failCleanup)return Promise.reject('Cannot remove new copy');t.disk.delete(args.slug);}
          if(name==='rename_paper_folder')return Promise.resolve(args.slug);
          return Promise.resolve(null);
        }};
      window.__TAURI_INTERNALS__={invoke:(name,args)=>t.invoke(name,args)};
      const pinia=createPinia(),app=createApp({render:()=>h(ImportQueue)});app.use(pinia);app.use(i18n);
      t.store=useImportStore(pinia);t.locale=value=>i18n.global.locale.value=value;
      t.start=()=>{const paths=Array.from({length:15},(_,i)=>'/fake/'+(i===3?'broken':i===5?'duplicate':i===7?'uncertain':'图书-'+i)+'.epub');
        t.batch=t.store.importFiles(paths,'leaf');};app.mount('#app');t.ready=true;
      </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true});page=await browser.newPage({viewport:{width:500,height:650}})
  page.on('pageerror',e=>errors.push(e.message))
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
})
after(async()=>{await browser?.close();await server?.close()})
async function open(){await page.goto(origin+'/__imports');await page.waitForFunction(()=>window.__test?.ready)}

test('a fifteen-book batch remains inspectable while a duplicate waits, and Skip removes just that copy',async()=>{
  await open();await page.evaluate(()=>window.__test.start())
  await page.waitForFunction(()=>window.__test.questions.length===1,null,{timeout:10000}).catch(async error=>{
    console.error(await page.evaluate(()=>({jobs:window.__test.store.jobs,calls:window.__test.calls,html:document.body.innerHTML.slice(0,500)})))
    throw error
  })
  assert.equal(await page.evaluate(()=>window.__test.calls.filter(c=>c.name==='import_ebook').length),15)
  await page.locator('summary').click()
  assert.equal(await page.locator('.queue-panel li').count(),15)
  await page.locator('li[data-status="error"]').waitFor()
  assert.match(await page.locator('li[data-status="error"]').textContent(),/broken.epub.*Damaged/s)
  const dialog=await page.evaluate(()=>window.__test.questions[0].args)
  assert.match(dialog.message,/duplicate.epub/)
  assert.ok(JSON.stringify(dialog.buttons).includes('跳过导入'))
  await page.evaluate(()=>window.__test.questions[0].resolve('跳过导入'))
  await page.waitForFunction(()=>window.__test.store.activeCount===0)
  assert.match(await page.locator('li[data-status="skipped"]').textContent(),/duplicate.epub.*已跳过/s)
  assert.match(await page.locator('.queue-warning').textContent(),/保留原名称/)
  assert.equal(await page.evaluate(()=>window.__test.disk.has('duplicate.epub')),false)
  assert.equal(await page.evaluate(()=>window.__test.calls.some(c=>c.name==='delete_paper'&&c.args.slug==='original')),false)
  assert.match(await page.locator('summary').textContent(),/15\/15/)
})

test('queue popovers stay inside narrow light/dark windows and translate statuses',async()=>{
  await page.setViewportSize({width:360,height:560});await page.evaluate(()=>document.documentElement.dataset.theme='dark')
  await page.waitForFunction(()=>document.querySelector('.queue-panel').getBoundingClientRect().right<=innerWidth)
  const box=await page.locator('.queue-panel').boundingBox()
  assert.ok(box.x>=0&&box.x+box.width<=360)
  assert.ok(box.y>=0&&box.y+box.height<=560,JSON.stringify(await page.evaluate(()=>({box:document.querySelector('.queue-panel').getBoundingClientRect().toJSON(),style:document.querySelector('.queue-panel').getAttribute('style'),innerWidth,innerHeight,summary:document.querySelector('summary').getBoundingClientRect().toJSON()}))))
  await page.evaluate(()=>window.__test.locale('en'))
  assert.match(await page.locator('li[data-status="skipped"]').textContent(),/Skipped/)
  assert.match(await page.locator('summary').textContent(),/Import 15\/15/)
  await page.getByRole('button',{name:'Clear finished items'}).click()
  assert.equal(await page.locator('.import-queue').count(),0)
  assert.deepEqual(errors,[])
})
