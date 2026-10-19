// Real reading hooks + activity store in separate windows; local IPC only.
import {test,before,after} from 'node:test'
import assert from 'node:assert/strict'
import {readFile} from 'node:fs/promises'
import {createServer} from 'vite'
import vue from '@vitejs/plugin-vue'
import ts from 'typescript'
const {chromium}=await import(process.env.ARGUS_TEST_PLAYWRIGHT||'playwright')
const source=await readFile(new URL('../src/views/MainView.vue',import.meta.url),'utf8')
const ast=ts.createSourceFile('Main.ts',source.slice(source.indexOf('>')+1,source.indexOf('</script>')),ts.ScriptTarget.Latest,true)
const main=ts.transpileModule(['activePaperInfo','readingPaperInfo','syncActivityReading'].map(name=>ast.statements.find(n=>ts.isFunctionDeclaration(n)&&n.name?.text===name).getText(ast)).join('\n'),{compilerOptions:{target:ts.ScriptTarget.ES2021}}).outputText
let server,browser,context,origin,now, disk=new Map(),pages=[]
const copy=x=>JSON.parse(JSON.stringify(x)),empty=()=>({version:2,days:{}})
const total=(root,slug)=>Object.values(disk.get(root)?.days||{}).flatMap(d=>Object.values(d.devices)).reduce((n,s)=>n+(slug?s.paper_reading_ms?.[slug]?.duration_ms??0:s.reading_ms??0),0)
const mocks={
 '@tauri-apps/api/core':`export const invoke=(name,args)=>window.readingInvoke(name,args);`,
 '@tauri-apps/api/event':`export const listen=async(name,fn)=>{const set=window.__r.listeners.get(name)||new Set();window.__r.listeners.set(name,set);set.add(fn);return()=>set.delete(fn)};`,
 '@tauri-apps/api/window':`export const getCurrentWindow=()=>({isFocused:()=>window.__r.focusPromise||Promise.resolve(window.__r.focused),onFocusChanged:async fn=>{window.__r.native.add(fn);return()=>window.__r.native.delete(fn)}});`,
 '@tauri-apps/api/webviewWindow':`export const getCurrentWebviewWindow=()=>({label:window.__r.label});`,
}
before(async()=>{
 server=await createServer({configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',plugins:[{name:'reading-mocks',enforce:'pre',resolveId:s=>s in mocks?'\0reading:'+s:null,load:id=>id.startsWith('\0reading:')?mocks[id.slice(9)]:null},vue()],server:{host:'127.0.0.1',port:0},logLevel:'error'})
 server.middlewares.use(async(req,res,next)=>{
  if(!req.url.startsWith('/__reading'))return next()
  res.setHeader('Content-Type','text/html');res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div><script type="module">
  import{createApp,h,ref,reactive,computed,watch,onMounted,onUnmounted}from'vue';import{createPinia}from'pinia';
  import{useActivityStore}from'/src/stores/activity.ts';import{usePaperAiReading}from'/src/composables/usePaperAiReading.ts';import{useReadingWindowFocus}from'/src/composables/useReadingWindowFocus.ts';
  const query=new URLSearchParams(location.search),mode=query.get('mode'),slug=ref(query.get('slug')||'one');
  const r=window.__r={listeners:new Map(),native:new Set(),focused:false,now:Number(query.get('now')),root:'library',label:mode==='main'?'main':'paper-ai-'+slug.value,timers:new Map(),emit(name,payload){for(const fn of r.listeners.get(name)||[])fn({payload})},focus(value){r.focused=value;for(const fn of r.native)fn({payload:value})},tick(){for(const fn of r.timers.values())fn()}};
  if(query.has('hold-focus'))r.focusPromise=new Promise(resolve=>r.resolveInitialFocus=resolve);
  const NativeDate=Date;window.Date=class extends NativeDate{constructor(...args){super(...(args.length?args:[r.now]))}static now(){return r.now}};
  const originalInterval=window.setInterval,originalClear=window.clearInterval;let timer=-1;
  window.setInterval=(fn,ms,...args)=>{if(ms!==15000)return originalInterval(fn,ms,...args);r.timers.set(timer,fn);return timer--};window.clearInterval=id=>r.timers.has(id)?r.timers.delete(id):originalClear(id);
  const app=createApp({setup(){const activityStore=useActivityStore();r.store=activityStore;
   if(mode==='main'){
    const readingWindowFocused=useReadingWindowFocus(),rightSidebarVisible=ref(true),sidebarTab=ref('ai');
    const libraryStore=reactive({currentPath:'library',papers:[{slug:'one',title:'First',file_type:'pdf'},{slug:'two',title:'Second',file_type:'pdf'}]});
    const readerStore=reactive({activeSlug:'one',openSlug:'one',tabs:[{slug:'one',title:'First',fileType:'pdf'}]});const selectionStore=reactive({selectedSlug:'one'});const activeTabKind=computed(()=>readerStore.activeSlug?'paper':null);
    ${main}
    watch([readingWindowFocused,rightSidebarVisible,sidebarTab,()=>selectionStore.selectedSlug,()=>readerStore.activeSlug],syncActivityReading,{flush:'sync'});
    r.sidebarOnly=value=>{readerStore.activeSlug=null;readerStore.openSlug=null;selectionStore.selectedSlug=value};r.hideSidebar=()=>rightSidebarVisible.value=false;
    onMounted(async()=>{activityStore.startHeartbeat();await activityStore.load('library');syncActivityReading();r.ready=true});onUnmounted(()=>activityStore.stopHeartbeat());
   }else{usePaperAiReading(slug);watch(()=>activityStore.libraryPath,path=>{if(path)r.ready=true})}
   return()=>h('div',activityStore.activeSession?.slug||'paused')}});app.use(createPinia());app.mount('#app');r.unmount=()=>app.unmount();
  </script>`))
 })
 await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port;browser=await chromium.launch({channel:'chrome',headless:true})
})
after(async()=>{await context?.close();await browser?.close();await server?.close()})
async function reset(){await context?.close();context=await browser.newContext();disk=new Map();pages=[];now=Date.now()}
async function open(mode='popup',slug='one',holdFocus=false){
 const page=await context.newPage();pages.push(page)
 await page.exposeBinding('readingInvoke',async(_source,name,args)=>{
  if(name==='get_device_id')return'machine'
  if(name==='get_current_library')return page.evaluate(()=>window.__r.root)
  if(name==='get_paper_meta')return{title:args.slug==='one'?'First':'Second',file_type:'pdf'}
  if(name==='get_activity_log')return copy(disk.get(args.root)||empty())
  if(name==='save_activity_log'){
   const value=copy(disk.get(args.root)||empty());for(const[date,day]of Object.entries(args.data.days)){value.days[date]??={date,devices:{}};if(day.devices[args.deviceId])value.days[date].devices[args.deviceId]=copy(day.devices[args.deviceId])}disk.set(args.root,value)
   await Promise.all(pages.filter(p=>!p.isClosed()).map(p=>p.evaluate(payload=>window.__r?.emit('activity-log-saved',payload),{root:args.root,writer:args.deviceId}).catch(()=>{})));return
  }throw Error(name)
 })
 await page.goto(origin+'/__reading?mode='+mode+'&slug='+slug+'&now='+now+(holdFocus?'&hold-focus':''));await page.waitForFunction(()=>window.__r?.ready);await page.evaluate(value=>window.__r.now=value,now);return page
}
async function focus(page,value){await page.evaluate(value=>window.__r.focus(value),value)}
async function advance(ms){now+=ms;await Promise.all(pages.filter(p=>!p.isClosed()).map(p=>p.evaluate(value=>{window.__r.now=value;window.__r.tick()},now)));await new Promise(r=>setTimeout(r,80))}
async function settle(){await new Promise(r=>setTimeout(r,100))}

test('foreground handoff between PDF/AI sidebar and standalone counts one paper once; background pauses',async()=>{
 await reset();const main=await open('main'),popup=await open();await focus(popup,false);await focus(main,true);await advance(10000);await focus(main,false);await settle()
 await focus(popup,true);await advance(30000);assert.equal(total('library','one'),40000)
 await focus(popup,false);await advance(20000);assert.equal(total('library','one'),40000)
 await main.evaluate(()=>window.__r.sidebarOnly('two'));await focus(main,true);await advance(12000);await focus(main,false);await settle()
 assert.equal(total('library','two'),12000);assert.equal(total('library'),52000)
 await main.evaluate(()=>window.__r.hideSidebar());await focus(main,true);await advance(10000);assert.equal(total('library'),52000)
})

test('standalone-only reading survives main being absent, a library switch and unmount',async()=>{
 await reset();const popup=await open();await focus(popup,true);await advance(25000)
 await popup.evaluate(()=>{window.__r.root='other-library';window.__r.emit('library-changed','other-library')});await popup.waitForFunction(()=>window.__r.store.libraryPath==='other-library'&&window.__r.store.activeSession)
 await advance(15000);await popup.evaluate(()=>window.__r.unmount());await settle()
 assert.equal(total('library','one'),25000);assert.equal(total('other-library','one'),15000)
 await advance(60000);assert.equal(total('other-library'),15000)
})


test('late initial focus samples cannot undo a new focus event, and hidden windows pause',async()=>{
 await reset();const popup=await open('popup','one',true)
 await focus(popup,true);await popup.evaluate(()=>window.__r.resolveInitialFocus(false));await advance(9000)
 assert.equal(total('library','one'),9000)
 await popup.evaluate(()=>{window.__r.visibility='hidden';Object.defineProperty(document,'visibilityState',{configurable:true,get:()=>window.__r.visibility});document.dispatchEvent(new Event('visibilitychange'))});await settle();await advance(6000)
 assert.equal(total('library','one'),9000)
 await popup.evaluate(()=>{window.__r.visibility='visible';document.dispatchEvent(new Event('visibilitychange'))});await advance(1000)
 assert.equal(total('library','one'),10000)
})
