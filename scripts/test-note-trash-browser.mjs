// Real note surfaces, with a small editor stand-in and local desktop/disk mocks.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors=[]
const mocks={
  '@tauri-apps/api/core': `export const invoke=(command,args)=>window.__test.invoke(command,args); export const convertFileSrc=p=>p; export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,handler)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(handler);return ()=>set.delete(handler)};export const emit=async(name,payload)=>window.__test.emit(name,payload);`,
  '@tauri-apps/plugin-dialog': `export const confirm=async()=>window.__test.confirm;`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'main'});`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'note-window-test',setTitle:async()=>{},onCloseRequested:async()=>()=>{}});`,
}
const editor=`import {defineComponent,h,ref} from 'vue';export default defineComponent({props:['initialContent'],emits:['change'],setup(props,{emit,expose}){const text=ref(props.initialContent);expose({setContent:value=>{text.value=value;return true}});return()=>h('textarea',{class:'mock-editor',value:text.value,onInput:event=>{text.value=event.target.value;emit('change',text.value)},style:'width:100%;height:100%;box-sizing:border-box;background:transparent;color:inherit;padding:16px;resize:none;border:0'})}});`
before(async()=>{
  server=await createServer({configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',plugins:[{
    name:'notes-desktop-mocks',enforce:'pre',resolveId:(source,importer)=>source in mocks?'\0mock:'+source:source.endsWith('/VditorEditor.vue')||source==='../VditorEditor.vue'?'\0mock-editor.js':null,
    load:id=>id==='\0mock-editor.js'?editor:id.startsWith('\0mock:')?mocks[id.slice(6)]:null,
  },vue()],optimizeDeps:{entries:['src/components/tabs/NotesTab.vue','src/views/NoteWindowView.vue']},server:{host:'127.0.0.1',port:0},logLevel:'error'})
  server.middlewares.use(async(req,res,next)=>{
    if(!req.url.startsWith('/__note-test'))return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><body><div id="app"></div><style>body{margin:0}#app{height:100vh}.test-panel{width:380px;height:100vh;background:var(--bg-primary)}.mock-editor{font:14px/1.6 sans-serif}</style><script type="module">
      import{createApp,h,ref}from'vue';import{createPinia}from'pinia';import{createI18n}from'vue-i18n';
      import{addCollection}from'@iconify/vue';import fluent from'@iconify-json/fluent/icons.json';
      import NotesTab from'/src/components/tabs/NotesTab.vue';import NoteWindow from'/src/views/NoteWindowView.vue';
      import{useLibraryStore}from'/src/stores/library.ts';import{zh,en}from'/src/i18n/locales.ts';
      import'/src/assets/main.css';import'/src/assets/themes.css';addCollection(fluent);
      const now=Date.now(),day=86400000,ts=new Date(now-5*day).toISOString();
      const note=(id,title,content)=>({id,title,content,created_at:ts,updated_at:ts});
      const live=[note('n1','关键方法：先定义任务，再检查证据','# 原始笔记\\n\\n**完整保留的正文**和公式 $x^2$。'),note('n2','我的阅读问题','尚待理解的背景知识')];
      const deleted=[{...note('d1','已删除的实验比较与局限分析笔记','## 保留的实验记录\\n\\n可恢复的完整内容。'),deleted_at:new Date(now-3*day).toISOString(),expires_at:new Date(now+27*day).toISOString()}];
      const slug=ref('paper'),mode=new URLSearchParams(location.search).get('mode');
      const test=window.__test={live,deleted,root:'/mock-library',confirm:true,listeners:new Map(),calls:[],
        emit(name,payload){for(const handler of test.listeners.get(name)||[])handler({payload})},
        changed(noteId,action,root=test.root){test.emit('paper-notes-changed',{root,slug:'paper',noteId,action})},
        switchPaper:value=>slug.value=value,
        invoke(command,args){
          test.calls.push({command,args:args?JSON.parse(JSON.stringify(args)):args});
          if(args?.expectedRoot&&args.expectedRoot!==test.root)return Promise.reject('Library changed');
          if(command==='get_current_library')return Promise.resolve(test.root);
          if(command==='list_notes'){
            const snapshot=args.slug==='paper'?live.map(({content,...meta})=>({...meta})):[];
            if(test.delayList)return new Promise(resolve=>test.releaseList=()=>resolve(snapshot));
            return Promise.resolve(snapshot);
          }
          if(command==='list_deleted_notes'){
            for(let i=deleted.length-1;i>=0;i--)if(Date.parse(deleted[i].expires_at)<=Date.now())deleted.splice(i,1);
            return Promise.resolve(args.slug==='paper'?JSON.parse(JSON.stringify(deleted)):[]);
          }
          if(command==='get_note'){
            const value=live.find(n=>n.id===args.noteId);return value?Promise.resolve(value.content):Promise.reject('Note no longer exists');
          }
          if(command==='delete_note'){
            const index=live.findIndex(n=>n.id===args.noteId);if(index<0)return Promise.reject('Note no longer exists');
            deleted.unshift({...live.splice(index,1)[0],deleted_at:new Date().toISOString(),expires_at:new Date(Date.now()+30*day).toISOString()});
            test.changed(args.noteId,'deleted');return Promise.resolve();
          }
          if(command==='restore_note'){
            if(test.failRestore)return Promise.reject('Cannot restore note');
            const index=deleted.findIndex(n=>n.id===args.noteId);if(index<0)return Promise.reject('Deleted note expired');
            const{deleted_at,expires_at,...rest}=deleted.splice(index,1)[0];live.push(rest);test.changed(args.noteId,'restored');return Promise.resolve({...rest});
          }
          if(command==='save_note'){
            const value=live.find(n=>n.id===args.noteId);if(!value)return Promise.reject('Note no longer exists');
            if(args.expectedContent!==undefined&&args.expectedContent!==value.content)return Promise.reject('Note changed elsewhere; draft preserved');
            value.content=args.content;value.updated_at=new Date().toISOString();return Promise.resolve();
          }
          if(command==='rename_note'){const n=live.find(n=>n.id===args.noteId);n.title=args.title;test.changed(n.id,'updated');return Promise.resolve()}
          if(command==='create_note'){const n=note('new-note','新建笔记','');live.push(n);test.changed(n.id,'created');return Promise.resolve({...n})}
          if(command==='read_note_asset')return Promise.reject('Missing image');
          return Promise.resolve(null);
        }};
      const pinia=createPinia(),i18n=createI18n({legacy:false,locale:'zh',messages:{zh,en}});
      const app=createApp({setup(){return()=>h('div',{class:'test-panel'},[mode==='window'?h(NoteWindow):h(NotesTab,{slug:slug.value})])}});app.use(pinia);app.use(i18n);
      const library=useLibraryStore(pinia);library.currentPath=test.root;library.refresh=async()=>{};
      test.switchRoot=value=>{test.root=value;library.currentPath=value;slug.value='other'};test.setLocale=value=>i18n.global.locale.value=value;
      localStorage.setItem('argus:note-window:note-window-test',JSON.stringify({slug:'paper',noteId:'n1',title:live[0].title,root:test.root}));
      app.mount('#app');test.mounted=true;
    </script></body>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true});page=await browser.newPage({viewport:{width:900,height:760}})
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
  page.on('pageerror',error=>errors.push(error.message))
})
after(async()=>{await browser?.close();await server?.close()})
async function open(mode='sidebar'){
  await page.goto(origin+'/__note-test?mode='+mode)
  await page.evaluate(()=>localStorage.clear());await page.reload()
  await page.waitForFunction(()=>window.__test?.mounted)
  await page.locator(mode==='window'?'.mock-editor':'.note-item').first().waitFor()
}
async function openEditor(){await page.locator('.note-item').first().click();await page.locator('.mock-editor').waitFor()}
const editorField=()=>page.locator('.mock-editor')
const trashToggle=()=>page.getByRole('button',{name:'最近删除',exact:false})

 test('Recently Deleted is a compact bottom section, expands, previews full content and restores original identity',async()=>{
  await open()
  assert.equal(await trashToggle().getAttribute('aria-expanded'),'false')
  assert.equal(await page.locator('.trash-item').count(),0)
  const panel=await page.locator('.notes-tab').boundingBox(),toggle=await trashToggle().boundingBox()
  assert.ok(Math.abs(panel.y+panel.height-toggle.y-toggle.height)<2)
  await trashToggle().click()
  await page.locator('.trash-note-title').click()
  await page.getByText('可恢复的完整内容。',{exact:true}).waitFor()
  assert.match(await page.locator('.trash-hint').textContent(),/30 天/)
  await page.getByRole('button',{name:'恢复「已删除的实验比较与局限分析笔记」',exact:true}).click()
  await page.waitForFunction(()=>window.__test.live.some(n=>n.id==='d1'))
  await page.locator('.note-item-title').filter({hasText:'已删除的实验比较'}).waitFor()
  assert.equal(await page.locator('.trash-item').count(),0)
  assert.equal(await page.evaluate(()=>window.__test.live.find(n=>n.id==='d1').content),'## 保留的实验记录\n\n可恢复的完整内容。')
})

test('manual deletion uses the native confirmation and both user rejection and restore failure preserve notes',async()=>{
  await open()
  await page.evaluate(()=>window.__test.confirm=false)
  await page.locator('.note-item').first().hover();await page.locator('.note-delete-btn').first().click()
  assert.equal(await page.evaluate(()=>window.__test.calls.filter(c=>c.command==='delete_note').length),0)
  await page.evaluate(()=>window.__test.confirm=true)
  await page.locator('.note-delete-btn').first().click()
  await page.waitForFunction(()=>window.__test.deleted.some(n=>n.id==='n1'))
  await trashToggle().click();assert.equal(await page.locator('.trash-item').count(),2)
  await page.evaluate(()=>window.__test.failRestore=true)
  await page.getByRole('button',{name:'恢复「关键方法：先定义任务，再检查证据」'}).click()
  await page.getByRole('alert').waitFor();assert.match(await page.getByRole('alert').textContent(),/Cannot restore/)
  assert.equal(await page.locator('.trash-item').count(),2)
  await page.evaluate(()=>window.__test.failRestore=false)
  await page.getByRole('button',{name:'恢复「关键方法：先定义任务，再检查证据」'}).click()
  await page.waitForFunction(()=>window.__test.live.some(n=>n.id==='n1'))
})

for(const mode of ['sidebar','window']){
  test(mode+': clean editors adopt AI changes, while dirty drafts cannot overwrite the AI saved version',async()=>{
    await open(mode);if(mode==='sidebar')await openEditor()
    await page.evaluate(()=>{const n=window.__test.live.find(n=>n.id==='n1');n.title='AI 修改后的标题';n.content='AI 修改后的正文';window.__test.changed('n1','updated')})
    await page.waitForFunction(()=>document.querySelector('.mock-editor').value==='AI 修改后的正文')
    await editorField().fill('我正在写的新草稿')
    await page.evaluate(()=>{window.__test.live.find(n=>n.id==='n1').content='AI 并发保存的新版';window.__test.changed('n1','updated')})
    await page.waitForFunction(()=>window.__test.calls.some(c=>c.command==='save_note'))
    assert.equal(await editorField().inputValue(),'我正在写的新草稿')
    assert.equal(await page.evaluate(()=>window.__test.live.find(n=>n.id==='n1').content),'AI 并发保存的新版')
    assert.ok(await page.getByText(/draft preserved/).count())
  })
  test(mode+': deleting an open dirty note pauses autosave and restoration keeps the draft',async()=>{
    await open(mode);if(mode==='sidebar')await openEditor()
    await editorField().fill('删除时仍保留的草稿')
    await page.evaluate(()=>window.__test.invoke('delete_note',{slug:'paper',noteId:'n1'}))
    const banner=page.locator(mode==='sidebar'?'.deleted-editor-banner':'.nw-deleted')
    await banner.waitFor();assert.equal(await editorField().inputValue(),'删除时仍保留的草稿')
    await editorField().fill('删除后继续保留的草稿')
    await page.waitForFunction(()=>window.__test.deleted.some(n=>n.id==='n1'))
    assert.equal(await page.evaluate(()=>window.__test.calls.filter(c=>c.command==='save_note').length),0)
    await banner.getByRole('button',{name:'恢复',exact:true}).click()
    await page.waitForFunction(()=>window.__test.live.find(n=>n.id==='n1')?.content==='删除后继续保留的草稿')
    assert.equal(await editorField().inputValue(),'删除后继续保留的草稿')
  })
}

test('late list reads and lifecycle events from another paper/library cannot change the new selection',async()=>{
  await open()
  await page.evaluate(()=>{window.__test.delayList=true;window.__test.changed('n1','updated')})
  await page.waitForFunction(()=>!!window.__test.releaseList)
  await page.evaluate(()=>{window.__test.delayList=false;window.__test.switchRoot('/other-library')})
  await page.waitForFunction(()=>document.querySelectorAll('.note-item').length===0)
  await page.evaluate(()=>{window.__test.releaseList();window.__test.changed('n1','deleted','/mock-library')})
  assert.equal(await page.locator('.note-item').count(),0)
  await trashToggle().click();assert.equal(await page.locator('.trash-item').count(),0)
})

test('narrow/dark and English layouts keep the trash and restore controls readable',async()=>{
  await open();await page.locator('.test-panel').evaluate(el=>el.style.width='285px')
  await page.evaluate(()=>document.documentElement.setAttribute('data-theme','dark'))
  await trashToggle().click();await page.locator('.trash-note-title').click()
  const restore=await page.locator('.trash-item .restore-btn').boundingBox(),panel=await page.locator('.notes-tab').boundingBox()
  assert.ok(restore.x+restore.width<=panel.x+panel.width)
  assert.ok(await page.locator('.trash-preview').isVisible())
  if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.locator('.test-panel').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/note-trash-dark.png'})}
  await page.evaluate(()=>window.__test.setLocale('en'))
  await page.getByText('Recently Deleted',{exact:true}).waitFor();assert.match(await page.locator('.trash-hint').textContent(),/30 days/)
  assert.deepEqual(errors,[])
})


test('late standalone startup data cannot replace an already edited draft',async()=>{
  await open('window')
  await editorField().fill('窗口打开后正在写的草稿')
  await page.evaluate(()=>window.__test.emit('note-window-data-note-window-test',{slug:'paper',noteId:'n1',title:'Delayed initial title'}))
  await page.waitForFunction(()=>window.__test.calls.filter(c=>c.command==='get_note').length>=2)
  assert.equal(await editorField().inputValue(),'窗口打开后正在写的草稿')
})
