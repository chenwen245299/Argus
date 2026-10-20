// Real paper/sidebar/standalone/canvas composers; isolated desktop and model mocks.
// Run with ARGUS_TEST_PLAYWRIGHT pointing to a Playwright index.mjs.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, context, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args); export const convertFileSrc=p=>p; export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,cb)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(cb);return()=>set.delete(cb)};
    export const emit=async(name,payload)=>window.__test.emit(name,payload); export const emitTo=async(target,name,payload)=>window.__test.emit(name,payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'main',onResized:async()=>()=>{},onFocusChanged:async()=>()=>{}});export class Window{static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'main'});export class WebviewWindow{static async getByLabel(){return null}}`,
}
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-paper-context', enforce: 'pre', resolveId: s => s in mocks ? '\0mock:' + s : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null }, vue()],
    optimizeDeps: { entries: ['src/components/tabs/AiTab.vue','src/components/tabs/CanvasChatTab.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__paper-context')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
    <style>html,body,#app,.test-panel{margin:0;width:100%;height:100%}</style><script type="module">
    import {createApp,h,ref} from 'vue';import {createPinia} from 'pinia';import {createI18n} from 'vue-i18n';
    import {addCollection} from '@iconify/vue';import fluent from '@iconify-json/fluent/icons.json';
    import AiTab from '/src/components/tabs/AiTab.vue';import CanvasChat from '/src/components/tabs/CanvasChatTab.vue';
    import {useAiStore} from '/src/stores/ai.ts';import {useReaderStore} from '/src/stores/reader.ts';
    import {useSettingsStore} from '/src/stores/settings.ts';import {zh,en} from '/src/i18n/locales.ts';
    import '/src/assets/main.css';import '/src/assets/themes.css';addCollection(fluent);
    const mode=new URLSearchParams(location.search).get('mode')||'panel',slug=ref('paper'),canvasId=ref('canvas');
    const stamp='2026-10-14T00:00:00Z';
    const seed={id:'paper-chat',slug:'paper',title:'Paper context test',createdAt:stamp,updatedAt:stamp,nodes:[
      {id:'u1',role:'user',content:'Original paper goal',createdAt:stamp},
      {id:'g1',role:'assistantGroup',promptId:'u1',createdAt:stamp,answers:[{id:'a1',providerId:'provider',providerName:'Example',modelId:'small',modelName:'Small model',content:'Earlier evidence and citations. '.repeat(80),createdAt:stamp}]},
      {id:'u2',role:'user',content:'Recent question',createdAt:stamp},
      {id:'g2',role:'assistantGroup',promptId:'u2',createdAt:stamp,answers:[{id:'a2',providerId:'provider',providerName:'Example',modelId:'small',modelName:'Small model',content:'Recent answer',createdAt:stamp}]}]};
    const canvasSeed={id:'canvas-chat',title:'Canvas context test',createdAt:stamp,updatedAt:stamp,messages:[
      {id:'u1',role:'user',content:'Original canvas goal',createdAt:stamp},
      {id:'a1',role:'assistant',content:'Earlier canvas evidence. '.repeat(80),createdAt:stamp},
      {id:'u2',role:'user',content:'Recent canvas question',createdAt:stamp},{id:'a2',role:'assistant',content:'Recent canvas answer',createdAt:stamp}]};
    const t=window.__test={listeners:new Map(),calls:[],manual:[],auto:[],requests:[],saved:[],root:'/mock-library',autoCompact:false,holdAuto:false,
      emit(name,payload){for(const cb of t.listeners.get(name)||[])cb({payload})},
      usage(args,messages){const windowSize=args.modelId==='large'?20000:10000;const tokens=500+messages.reduce((n,m)=>n+Math.ceil(JSON.stringify(m).length/4),0);
        return{usedTokens:tokens,contextWindow:windowSize,remainingTokens:windowSize-tokens,systemTokens:300,toolsTokens:200,messageTokens:tokens-500,
          reservedTokens:1000,thresholdTokens:windowSize*.8,estimated:true,capacityEstimated:false,mediaEstimated:false,canCompact:messages.length>=3,providerId:args.providerId,modelId:args.modelId}},
      prepare(args,compacted=false){const raw=args.messages.filter(m=>m.role!=='system');const messages=compacted?[{role:'user',content:'<compacted-summary>Goal and exact evidence</compacted-summary>'},...raw.slice(-2)]:raw;
        return{messages,usage:t.usage(args,messages),compacted,summary:compacted?'Goal and evidence':null}},
      invoke(name,args){t.calls.push({name,args});
        if(name==='get_current_library')return Promise.resolve(t.root);
        if(name==='get_ai_settings')return Promise.resolve(JSON.parse(JSON.stringify(t.aiSettings)));
        if(name==='get_paper_ai_conversations')return Promise.resolve(JSON.parse(localStorage.getItem(t.root+':paper:'+args.slug)||'null')||(args.slug==='paper'?[seed]:[]));
        if(name==='get_canvas_ai_conversations')return Promise.resolve(JSON.parse(localStorage.getItem(t.root+':canvas:'+args.canvasId)||'null')||[canvasSeed]);
        if(name==='get_paper_meta')return Promise.resolve({title:'Paper title',abstract:'A complete abstract'});
        if(name==='get_chat_history'||name==='get_paper_chat')return Promise.resolve([]);
        if(name==='prepare_library_chat_context'){
          if(args.mode==='manual')return new Promise((resolve,reject)=>t.manual.push({args,resolve,reject}));
          if(args.mode==='auto'&&t.holdAuto)return new Promise((resolve,reject)=>t.auto.push({args,resolve,reject}));
          return Promise.resolve(t.prepare(args,args.mode==='auto'&&t.autoCompact));
        }
        if(name==='chat_with_library')return new Promise((resolve,reject)=>t.requests.push({args,resolve,reject}));
        if(name==='save_paper_ai_conversations'){const snapshot=JSON.parse(JSON.stringify(args));t.saved.push({kind:'paper',...snapshot});localStorage.setItem(t.root+':paper:'+args.slug,JSON.stringify(snapshot.conversations))}
        if(name==='save_canvas_ai_conversations'){const snapshot=JSON.parse(JSON.stringify(args));t.saved.push({kind:'canvas',...snapshot});localStorage.setItem(t.root+':canvas:'+args.canvasId,JSON.stringify(snapshot.conversations))}
        return Promise.resolve(null);
      },
      compact(i){t.manual[i].resolve(t.prepare(t.manual[i].args,true))},
      finish(i,text='New answer'){t.requests[i].resolve(text)},
      switchPaper(value){slug.value=value},switchCanvas(value){canvasId.value=value},
      switchLibrary(value){t.root=value;t.emit('library-changed',value)},
    };
    const app=createApp({setup(){return()=>h('div',{class:'test-panel'},[mode==='canvas'?h(CanvasChat,{canvasId:canvasId.value}):h(AiTab,{slug:slug.value,standalone:mode==='standalone'})])}});
    const pinia=createPinia();app.use(pinia);app.use(createI18n({legacy:false,locale:'zh',messages:{zh,en}}));
    const ai=useAiStore(pinia);ai.loaded=true;ai.settings={default_provider_id:'provider',default_model_id:'small',providers:[{id:'provider',name:'Example',enabled:true,kind:'custom',base_url:'https://example.test/v1',models:[
      {id:'small',display_name:'Small model',context_length:10000,capabilities:['vision','tool_calling']},
      {id:'large',display_name:'Large model',context_length:20000,capabilities:['vision','tool_calling']}]}]};ai.load=async()=>{};
    t.aiSettings=JSON.parse(JSON.stringify(ai.settings));
    useSettingsStore(pinia).load=async()=>{};useReaderStore(pinia).readingStateFor=()=>({page:4});
    app.mount('#app');t.mounted=true;
    </script>`))
  })
  await server.listen();origin=`http://127.0.0.1:${server.httpServer.address().port}`
  browser=await chromium.launch({channel:'chrome',headless:true})
  context=await browser.newContext({viewport:{width:420,height:850}})
  await context.route('**/*',r=>r.request().url().startsWith(origin)?r.continue():r.abort())
  page=await context.newPage();page.on('pageerror',e=>errors.push(e.message))
})
after(async()=>{await browser?.close();await server?.close()})
async function open(mode='panel',reset=true){
  await page.setViewportSize({width:mode==='standalone'?850:420,height:850})
  await page.goto(origin+'/__paper-context?mode='+mode)
  if(reset){await page.evaluate(()=>localStorage.clear());await page.reload()}
  await page.locator('.context-trigger').waitFor({timeout:15000})
}
const composer=()=>page.locator('.composer-input,.cc-input').last()
const manual=n=>page.waitForFunction(n=>window.__test.manual.length===n,n)
const requests=n=>page.waitForFunction(n=>window.__test.requests.length===n,n)
const hasCheckpoint=kind=>page.evaluate(kind=>window.__test.saved.some(s=>s.kind===kind&&s.conversations.some(c=>Object.values(c.contextStates||{}).some(state=>state.checkpoint))),kind)
async function waitCheckpoint(kind){await page.waitForFunction(kind=>window.__test.saved.some(s=>s.kind===kind&&s.conversations.some(c=>Object.values(c.contextStates||{}).some(state=>state.checkpoint))),kind)}
async function send(text){await composer().fill(text);await composer().press('Enter')}
async function chooseModel(name){await page.locator('.model-trigger').click();await page.locator('.model-row').filter({hasText:name}).click()}

test('paper sidebar shows the ring and exact scope; manual compaction preserves the draft and visible history',async()=>{
  await open();await composer().fill('Unsent draft')
  await page.locator('.context-trigger').hover();await page.locator('.context-panel').waitFor()
  assert.match(await page.locator('.context-panel').innerText(),/10,000 tokens/)
  assert.equal(await page.evaluate(()=>window.__test.manual.length),0)
  const inspect=await page.evaluate(()=>window.__test.calls.find(c=>c.name==='prepare_library_chat_context').args)
  assert.equal(inspect.paperSlug,'paper');assert.equal(inspect.currentPage,4);assert.equal(inspect.plainFallback,false)
  const bubbles=await page.locator('.user-bubble').count()
  await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.compact(0));await waitCheckpoint('paper')
  assert.equal(await composer().inputValue(),'Unsent draft')
  assert.equal(await page.locator('.user-bubble').count(),bubbles)
})

test('paper checkpoints survive reload and are used by automatic preparation and the live agent',async()=>{
  await open();await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.compact(0));await waitCheckpoint('paper')
  await open('panel',false);await send('Continue');await requests(1)
  const args=await page.evaluate(()=>window.__test.requests[0].args)
  assert.equal(args.manageContext,true);assert.equal(args.expectedRoot,'/mock-library');assert.equal(args.paperSlug,'paper')
  assert.match(args.messages[0].content,/compacted-summary/)
  assert.equal(args.messages.at(-1).content,'Continue')
  assert(!args.messages.some(m=>m.content==='Original paper goal'))
  await page.evaluate(()=>window.__test.finish(0))
})

test('stopping paper compaction refuses its late checkpoint and pauses queued input',async()=>{
  await open();await page.locator('.context-trigger').click();await manual(1)
  await send('Queued during compression')
  await page.getByRole('button',{name:'停止',exact:true}).click()
  await page.evaluate(()=>window.__test.compact(0))
  await page.waitForFunction(()=>window.__test.calls.some(c=>c.name==='cancel_ai_request'))
  assert.equal(await hasCheckpoint('paper'),false)
  assert.equal(await page.evaluate(()=>window.__test.requests.length),0)
})

test('paper compaction failure retains history and is shown in the meter',async()=>{
  await open();await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.manual[0].reject('Summary unavailable'))
  await page.locator('.context-trigger').hover();await page.locator('.context-error').waitFor()
  assert.match(await page.locator('.context-error').innerText(),/Summary unavailable/)
  assert.equal(await hasCheckpoint('paper'),false)
})

test('models keep separate capacities and checkpoints; regenerating before a boundary rejects it',async()=>{
  await open();await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.compact(0));await waitCheckpoint('paper')
  await chooseModel('Large model');await page.locator('.context-trigger').hover()
  await page.waitForFunction(()=>document.querySelector('.context-total')?.textContent.includes('20,000'))
  await send('Use another model');await requests(1)
  let args=await page.evaluate(()=>window.__test.requests[0].args)
  assert.equal(args.modelId,'large');assert.equal(args.messages[0].content,'Original paper goal')
  await page.evaluate(()=>window.__test.finish(0));await page.getByRole('button',{name:'重新生成',exact:true}).last().waitFor()
  await chooseModel('Small model')
  await page.getByRole('button',{name:'重新生成',exact:true}).first().click();await requests(2)
  args=await page.evaluate(()=>window.__test.requests[1].args)
  assert.equal(args.messages[0].content,'Original paper goal')
  await page.evaluate(()=>window.__test.finish(1))
})

test('a library switch cancels paper compaction and rejects a late save',async()=>{
  await open();await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.switchLibrary('/other-library'))
  await page.evaluate(()=>window.__test.compact(0))
  await page.waitForFunction(()=>window.__test.calls.some(c=>c.name==='cancel_ai_request'))
  assert.equal(await hasCheckpoint('paper'),false)
})

test('live paper usage and native checkpoints are preserved without using cumulative billing totals',async()=>{
  await open();await page.evaluate(()=>window.__test.autoCompact=true);await send('New question');await requests(1)
  assert.match(await page.evaluate(()=>window.__test.requests[0].args.messages[0].content),/compacted-summary/)
  await page.evaluate(()=>{const t=window.__test,r=t.requests[0],native=[{role:'user',content:'<compacted-summary>Live evidence</compacted-summary>'},
    {role:'assistant',content:'',tool_calls:[{id:'call',type:'function',function:{name:'get_paper',arguments:'{}'}}]},
    {role:'tool',tool_call_id:'call',content:'Paper evidence'},{role:'assistant',content:'New answer'}];
    t.emit(r.args.eventName+'-context-checkpoint',{messages:native});
    t.emit(r.args.eventName+'-context-usage',{...t.usage(r.args,native),usedTokens:4000,remainingTokens:6000});
    t.emit(r.args.eventName+'-usage',{input_tokens:90000,output_tokens:100,total_tokens:90100})})
  assert.equal((await page.locator('.context-trigger').innerText()).trim(),'40%')
  await page.evaluate(()=>window.__test.finish(0))
  await waitCheckpoint('paper');await page.evaluate(()=>window.__test.autoCompact=false);await send('Follow up');await requests(2)
  const messages=await page.evaluate(()=>window.__test.requests[1].args.messages)
  assert(messages.some(m=>m.role==='tool'&&m.tool_call_id==='call'))
  assert.match(messages[0].content,/Live evidence/)
  await page.evaluate(()=>window.__test.finish(1))
})

test('stopping automatic preparation never starts the agent or saves its late summary',async()=>{
  await open();await page.evaluate(()=>window.__test.holdAuto=true)
  await send('Question during automatic compression')
  await page.waitForFunction(()=>window.__test.auto.length===1)
  await page.getByRole('button',{name:'停止',exact:true}).click()
  await page.evaluate(()=>{const t=window.__test;t.auto[0].resolve(t.prepare(t.auto[0].args,true))})
  await page.waitForFunction(()=>!document.querySelector('.stop-btn'))
  assert.equal(await hasCheckpoint('paper'),false)
  assert.equal(await page.evaluate(()=>window.__test.requests.length),0)
})

test('standalone and narrow dark sidebars keep the context meter in the tools row',async()=>{
  for(const mode of ['panel','standalone']){
    await open(mode);if(mode==='panel')await page.setViewportSize({width:350,height:850})
    await page.evaluate(()=>document.documentElement.dataset.theme='midnight')
    await page.locator('.context-trigger').focus();await page.locator('.context-panel').waitFor()
    const ring=await page.locator('.context-trigger').boundingBox(),send=await page.locator('.send-btn').last().boundingBox(),panel=await page.locator('.context-panel').boundingBox()
    const width=page.viewportSize().width
    const card=await page.locator('.composer-box').boundingBox()
    assert(ring.x>=0&&ring.x+ring.width<=width&&send.x+send.width<=width)
    assert(ring.y>=card.y&&ring.y+ring.height<=card.y+card.height)
    assert(Math.abs(ring.y+ring.height/2-send.y-send.height/2)<=1)
    assert.equal(await page.locator('.chat-run-stats').count(),mode==='standalone'?1:0)
    assert(panel.x>=0&&panel.x+panel.width<=width)
    if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/paper-context-'+mode+'.png'})}
  }
})

test('all chats retain usage, while only standalone paper chat shows statistics beside the tools',async()=>{
  for(const mode of ['panel','standalone','canvas']){
    await open(mode);if(mode==='panel')await page.setViewportSize({width:350,height:850})
    await send('Show measured statistics');await requests(1)
    await page.evaluate(()=>{const t=window.__test,r=t.requests[0],name=r.args.eventName;
      const sample={input_tokens:1000,output_tokens:200,total_tokens:1200,cache_hit_tokens:900,cache_reported:true,model_calls:3,model_duration_ms:2000};
      t.emit(name+'-run-usage',sample);t.emit(name+'-run-usage',sample);
      t.emit(name+'-compaction-usage',{id:'summary',input_tokens:500,output_tokens:50,total_tokens:550,model_calls:1,cache_hit_tokens:300,cache_reported:true});
      t.finish(0)
    })
    await page.waitForFunction(()=>!document.querySelector('.stop-btn,.cc-send.stop'))
    await page.waitForFunction(()=>window.__test.saved.some(s=>s.conversations.some(c=>Object.values(c.usageRuns||{}).some(run=>run.kind==='compaction'))))
    const runs=await page.evaluate(()=>Object.values(window.__test.saved.at(-1).conversations[0].usageRuns))
    assert.equal(runs.reduce((sum,run)=>sum+(run.totalTokens||0),0),1750)
    assert.equal(runs.reduce((sum,run)=>sum+(run.modelCalls||0),0),4)
    if(mode==='standalone'){
      const text=await page.locator('.chat-run-stats').textContent()
      assert.match(text,/3 轮 4 步/);assert.match(text,/1.8k tok/);assert.match(text,/~100 tok\/s/);assert.match(text,/缓存命中 80%/)
      assert.equal(await page.locator('.composer-toolbar .chat-run-stats').count(),1)
      const stats=await page.locator('.chat-run-stats').boundingBox(),sendBox=await page.locator('.send-btn').last().boundingBox()
      const tool=await page.locator('.composer-toolbar .toolbar-btn').first().boundingBox()
      assert(Math.abs(stats.y+stats.height/2-sendBox.y-sendBox.height/2)<=1)
      assert(Math.abs(stats.y+stats.height/2-tool.y-tool.height/2)<=1)
    }else assert.equal(await page.locator('.chat-run-stats').count(),0)
    if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/context-stats-'+mode+'.png'})}
  }
})

test('standalone statistics remain a single tools row with large totals and narrow windows',async()=>{
  await open('standalone');await send('Long conversation statistics');await requests(1)
  await page.evaluate(()=>{const t=window.__test,name=t.requests[0].args.eventName;
    t.emit(name+'-run-usage',{input_tokens:1600000,output_tokens:200000,total_tokens:1800000,cache_hit_tokens:1280000,cache_reported:true,model_calls:28,model_duration_ms:1449275});t.finish(0)
  })
  await page.waitForFunction(()=>!document.querySelector('.stop-btn'))
  for(const width of [1100,850,620,390]){
    await page.setViewportSize({width,height:850})
    await page.evaluate(()=>document.documentElement.dataset.theme='midnight')
    await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))))
    const stats=await page.locator('.chat-run-stats').boundingBox(),sendBox=await page.locator('.send-btn').last().boundingBox()
    const tool=await page.locator('.composer-toolbar .toolbar-btn').first().boundingBox()
    const content=await page.locator('.run-stats-content').boundingBox()
    assert(Math.abs(stats.y+stats.height/2-sendBox.y-sendBox.height/2)<=1)
    assert(Math.abs(stats.y+stats.height/2-tool.y-tool.height/2)<=1)
    assert(sendBox.x+sendBox.width<=width)
    assert(content.x>=stats.x-1&&content.width<=stats.width+1, 'visible statistics fit without clipping')
    assert.equal(await page.locator('.run-stats-content').evaluate(el=>el.scrollHeight<=20),true)
    if(width===1100){
      await page.locator('.run-speed').waitFor({state:'visible'})
      await page.locator('.run-cache').waitFor({state:'visible'})
      assert.match(await page.locator('.chat-run-stats').innerText(),/1.8M tok/)
    }
    if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.locator('.composer-box').screenshot({animations:'disabled',path:process.env.ARGUS_TEST_OUTPUT_DIR+'/context-stats-inline-'+width+'.png'})
  }
})

test('canvas meter compacts without changing the draft and retains its scoped instruction on follow-up',async()=>{
  await open('canvas');await composer().fill('Canvas draft');await page.locator('.context-trigger').click();await manual(1)
  const args=await page.evaluate(()=>window.__test.manual[0].args)
  assert.equal(args.canvasId,'canvas');assert.equal(args.plainFallback,false)
  await page.evaluate(()=>window.__test.compact(0));await waitCheckpoint('canvas')
  assert.equal(await composer().inputValue(),'Canvas draft')
  await send('Continue canvas');await requests(1)
  const request=await page.evaluate(()=>window.__test.requests[0].args)
  assert.equal(request.canvasId,'canvas');assert.equal(request.manageContext,true)
  assert.equal(request.messages[0].role,'system');assert.match(request.messages[0].content,/canvas/)
  assert.match(request.messages[1].content,/compacted-summary/)
  await page.evaluate(()=>window.__test.finish(0))
})

test('canvas switching while compacting cannot attach the checkpoint to the new canvas',async()=>{
  await open('canvas');await page.locator('.context-trigger').click();await manual(1)
  await page.evaluate(()=>window.__test.switchCanvas('other-canvas'))
  await page.evaluate(()=>window.__test.compact(0))
  await page.waitForFunction(()=>window.__test.calls.some(c=>c.name==='cancel_ai_request'))
  assert.equal(await hasCheckpoint('canvas'),false)
  assert.deepEqual(errors,[])
})

test('23 paper turns keep complete early evidence and saved figure references through a real send and reload',async()=>{
  await open('standalone')
  await page.evaluate(()=>{
    const stamp='2026-10-19T00:00:00Z',nodes=[]
    const preview='complete-paper-evidence '.repeat(4000)
    for(let i=1;i<=23;i++){
      nodes.push({id:'u'+i,role:'user',content:'Prior question '+i,createdAt:stamp})
      nodes.push({id:'g'+i,role:'assistantGroup',promptId:'u'+i,createdAt:stamp,answers:[{
        id:'a'+i,providerId:'provider',providerName:'Example',modelId:'small',modelName:'Small model',content:'Prior answer '+i,createdAt:stamp,
        steps:i===1?[{tool:'get_paper_fulltext',args:'paper',argsJson:'{"slug":"paper"}',ok:true,preview,chars:preview.length},
          {tool:'view_paper_page',args:'page 15',argsJson:'{"pages":[15]}',ok:true,preview:'Figure 4 caption',images:[{slug:'paper',page:15,file:'saved-figure.png'}]}]:undefined,
      }]})
    }
    localStorage.setItem('/mock-library:paper:paper',JSON.stringify([{id:'paper-chat',slug:'paper',title:'23-turn reading',createdAt:stamp,updatedAt:stamp,nodes}]))
  })
  await page.reload();await page.locator('.context-trigger').waitFor()
  await send('Question 24');await requests(1)
  const validate=async()=>{
    const messages=await page.evaluate(()=>window.__test.requests[0].args.messages)
    for(let i=1;i<=23;i++){
      assert(messages.some(m=>m.role==='user'&&m.content==='Prior question '+i))
      assert(messages.some(m=>m.role==='assistant'&&m.content==='Prior answer '+i))
    }
    const paper=messages.find(m=>m.role==='tool'&&m.content.startsWith('complete-paper-evidence'))
    assert.equal(paper.content.length,'complete-paper-evidence '.repeat(4000).length)
    const images=messages.flatMap(m=>Array.isArray(m.content)?m.content:[]).filter(p=>p.type==='image_url')
    assert.equal(images.length,1);assert.match(images[0].image_url.url,/^argus-chat-image:/)
  }
  await validate();await page.evaluate(()=>window.__test.finish(0))
  await page.waitForFunction(()=>window.__test.saved.some(s=>s.kind==='paper'&&s.conversations.some(c=>c.nodes.length>=48)))
  await page.reload();await page.locator('.context-trigger').waitFor()
  await send('Question 25');await requests(1);await validate()
  await page.evaluate(()=>window.__test.finish(0))
})
