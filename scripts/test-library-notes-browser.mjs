// Real list, toolbar, Markdown editor and approval card. Only library/desktop IO is mocked.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args);export const convertFileSrc=p=>p;export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,handler)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(handler);return()=>set.delete(handler)};export const emit=async(name,payload)=>window.__test.emit(name,payload);export const emitTo=emit;`,
  '@tauri-apps/plugin-dialog': `export const confirm=async()=>window.__test.confirm;export const save=async()=>null;export const open=async()=>null;`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'main',onResized:async()=>()=>{},onFocusChanged:async()=>()=>{},isFocused:async()=>true});export class Window{static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'main'});export class WebviewWindow{static async getByLabel(){return null}}`,
}
before(async()=>{
  server=await createServer({configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',cacheDir:'node_modules/.vite-library-notes-test',
    plugins:[{name:'library-note-io',enforce:'pre',resolveId:s=>s in mocks?'\0note-mock:'+s:null,load:id=>id.startsWith('\0note-mock:')?mocks[id.slice(11)]:null},vue()],
    optimizeDeps:{entries:['src/components/PaperList.vue','src/components/Toolbar.vue','src/components/MarkdownNoteView.vue','src/components/WriteConfirmCard.vue']},
    server:{host:'127.0.0.1',port:0},logLevel:'error'})
  server.middlewares.use(async(req,res,next)=>{
    if(!req.url.startsWith('/__library-notes'))return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html,body,#app{margin:0;width:100%;height:100%;overflow:hidden}body{user-select:none;-webkit-user-select:none}input,textarea,[contenteditable="true"]{user-select:text;-webkit-user-select:text}.workspace{height:100vh;display:flex;flex-direction:column}.workarea{flex:1;min-height:0;display:flex}.centre{flex:1;min-width:0;min-height:0}.test-sidebar{width:390px;flex-shrink:0;border-left:1px solid #ddd}.approval{position:fixed;bottom:10px;right:10px;width:440px;max-height:85vh;overflow:auto;z-index:10000;background:var(--bg-primary)}</style>
      <script type="module">
      import{createApp,h,ref}from'vue';import{createPinia}from'pinia';import{addCollection}from'@iconify/vue';import fluent from'@iconify-json/fluent/icons.json';
      import PaperList from'/src/components/PaperList.vue';import Toolbar from'/src/components/Toolbar.vue';import MarkdownNoteView from'/src/components/MarkdownNoteView.vue';import WriteConfirmCard from'/src/components/WriteConfirmCard.vue';
      import RightSidebar from'/src/components/RightSidebar.vue';
      import{useLibraryStore}from'/src/stores/library.ts';import{useCollectionsStore}from'/src/stores/collections.ts';import{useReaderStore}from'/src/stores/reader.ts';import{useSelectionStore}from'/src/stores/selection.ts';import{useAiStore}from'/src/stores/ai.ts';
      import{i18n}from'/src/i18n/index.ts';import'/src/assets/main.css';import'/src/assets/themes.css';import'vue-virtual-scroller/dist/vue-virtual-scroller.css';addCollection(fluent);
      const realStores=new URLSearchParams(location.search).has('realStores');
      const now=new Date().toISOString(),notes=new Map(),preview=ref(null),sidebarTab=ref('notes'),showSidebar=ref(false),attachments=new Map(),highlights=new Map(),chats=new Map();
      const t=window.__test={notes,attachments,highlights,confirm:true,listeners:new Map(),calls:[],root:'/mock-library',holdSave:false,holdScans:false,pendingScans:[],failCreate:false,
        emit(name,payload){for(const handler of t.listeners.get(name)||[])handler({payload})},
        async invoke(name,args){t.calls.push({name,args:args?JSON.parse(JSON.stringify(args)):args});
          if(args?.expectedRoot&&args.expectedRoot!==t.root)throw new Error('Library changed');
          if(name==='create_library_note'){if(t.failCreate)throw new Error('Mock creation failed');const slug='note_'+(notes.size+1),note={slug,id:slug,title:args.title,content:args.content,collectionId:args.collectionId,collectionName:'图书 / 读书笔记',createdAt:now};notes.set(slug,note);t.collectionFile.assignments.push({paper_id:slug,collection_id:args.collectionId});t.emit('library-notes-changed',{root:t.root,slug,action:'created'});return {...note}}
          if(name==='get_library_note'){if(!notes.has(args.slug))throw new Error('Note no longer exists');return {...notes.get(args.slug)}}
          if(name==='save_library_note'){
            if(t.holdSave)await new Promise(resolve=>t.releaseSave=resolve);
            const note=notes.get(args.expected.slug);if(!note||JSON.stringify(note)!==JSON.stringify(args.expected))throw new Error('Note changed; draft retained');
            const saved={...note,title:args.title,content:args.content};notes.set(note.slug,saved);t.emit('library-notes-changed',{root:t.root,slug:note.slug,action:'updated'});return {...saved};
          }
          if(name==='get_collections')return JSON.parse(JSON.stringify(t.collectionFile));
          if(name==='get_current_library')return t.root;
          if(name==='list_notes')return (attachments.get(args.slug)||[]).map(({content,...meta})=>meta);
          if(name==='list_deleted_notes')return [];
          if(name==='create_note'){const n={id:crypto.randomUUID(),title:'新建笔记',content:'',created_at:now,updated_at:now};attachments.set(args.slug,[...(attachments.get(args.slug)||[]),n]);return {...n}}
          if(name==='get_note')return (attachments.get(args.slug)||[]).find(n=>n.id===args.noteId)?.content||'';
          if(name==='save_note'){const n=attachments.get(args.slug).find(n=>n.id===args.noteId);n.content=args.content;return null}
          if(name==='get_highlights')return structuredClone(highlights.get(args.slug)||[]);
          if(name==='save_highlights'){highlights.set(args.slug,JSON.parse(JSON.stringify(args.highlights)));return null}
          if(name==='get_paper_ai_conversations')return structuredClone(chats.get(args.slug)||[]);
          if(name==='save_paper_ai_conversations'){chats.set(args.slug,JSON.parse(JSON.stringify(args.conversations)));return null}
          if(name==='get_chat_history')return [];
          if(name==='prepare_library_chat_context')return {messages:args.messages,compacted:false,usage:{usedTokens:200,contextWindow:32000,remainingTokens:31800,systemTokens:100,toolsTokens:50,messageTokens:50,reservedTokens:1000,thresholdTokens:25000,estimated:true,canCompact:false}};
          if(name==='chat_with_library')return '这是当前独立笔记的模拟 AI 回复。';
          if(name==='get_canvas_note_titles_map')return {};
          if(name==='list_papers_in_collection')return t.entries();
          if(name==='get_paper_meta')return {...notes.get(args.slug),authors:[],tags:[],file_type:'md'};
          if(name==='get_arxiv_status')return {fetching:false,analyzing:false};
          if(name==='get_ai_settings')return t.aiSettings;
          if(name==='get_settings')return {};
          if(name==='list_papers'||name==='scan_library'){const snapshot=t.entries();if(t.holdScans)return new Promise(resolve=>t.pendingScans.push(()=>resolve(snapshot)));return snapshot}
          if(name==='get_cn_holidays')return [];
          return null;
        }};
      window.__TAURI_INTERNALS__={invoke:(name,args)=>t.invoke(name,args)};
      t.collectionFile={collections:[{id:'top',name:'图书',parent_id:null,created_at:now},{id:'leaf',name:'读书笔记',parent_id:'top',created_at:now}],assignments:[]};
      t.entries=()=>[...notes.values()].map(n=>({slug:n.slug,id:n.id,title:n.title,authors:[],tags:[],added_at:n.createdAt,reading_status:'unread',file_type:'md',status:{text_extracted:false,metadata_fetched:false,ai_summary_done:false,vectorized:false}}));
      const pinia=createPinia();const app=createApp({setup(){return()=>h('div',{class:'workspace'},[
        h(Toolbar),h('div',{class:'workarea'},[h('div',{class:'centre'},[t.reader.activeSlug?h(MarkdownNoteView,{slug:t.reader.activeSlug,key:t.reader.activeSlug}):h(PaperList)]),showSidebar.value?h('div',{class:'test-sidebar'},[h(RightSidebar,{activeTab:sidebarTab.value})]):null]),
        preview.value?h('div',{class:'approval'},[h(WriteConfirmCard,{preview:preview.value,onApprove:()=>{t.approvals++;preview.value=null},onReject:()=>{t.rejections++;preview.value=null}})]):null])}});
      app.use(pinia);app.use(i18n);t.library=useLibraryStore(pinia);t.reader=useReaderStore(pinia);t.selection=useSelectionStore(pinia);
      t.library.currentPath=t.root;t.library.isLoading=false;if(!realStores)t.library.refresh=async()=>{t.library.papers=t.entries()};
      const collections=useCollectionsStore(pinia);collections.file=JSON.parse(JSON.stringify(t.collectionFile));if(!realStores)collections.load=async()=>{collections.file=JSON.parse(JSON.stringify(t.collectionFile))};
      t.collections=collections;
      t.listeners.set('library-notes-changed',new Set([({payload})=>{if(payload.root===t.root)void Promise.all([t.library.refresh(),collections.load()])}]));
      t.selection.selectNav('collection:leaf');const ai=useAiStore(pinia);ai.loaded=true;ai.settings={default_provider_id:'mock',default_model_id:'mock-model',providers:[{id:'mock',name:'Mock',enabled:true,kind:'custom',base_url:'https://example.test/v1',models:[{id:'mock-model',display_name:'Mock model',context_length:32000,capabilities:['tool_calling','vision']}]}]};ai.load=async()=>{};t.aiSettings=JSON.parse(JSON.stringify(ai.settings));
      t.setSidebar=tab=>{sidebarTab.value=tab;showSidebar.value=true};
      t.back=()=>t.reader.closePaper();t.approvals=0;t.rejections=0;t.preview=tool=>{const note=notes.values().next().value;preview.value={tool,slug:note.slug,paperTitle:note.collectionName,noteTitle:note.title,content:tool==='update_library_note'?'修改后的正文':note.content,duplicateTitle:false,noteBefore:tool==='update_library_note'?{id:note.id,title:note.title,content:note.content,created_at:now,updated_at:now}:undefined}};
      t.external=content=>{const note=notes.values().next().value;note.content=content;t.emit('library-notes-changed',{root:t.root,slug:note.slug,action:'updated'})};
      t.setLocale=locale=>i18n.global.locale.value=locale;app.mount('#app');t.ready=true;
      </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true});page=await browser.newPage({viewport:{width:1400,height:900}})
  page.setDefaultTimeout(10000)
  page.on('pageerror',e=>{errors.push(e.message);console.error(e.message)})
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
})
after(async()=>{await browser?.close();await server?.close()})
async function open(realStores=false){await page.goto(origin+'/__library-notes'+(realStores?'?realStores=1':''));await page.waitForFunction(()=>window.__test?.ready);if(realStores){await page.evaluate(()=>localStorage.clear());await page.reload();await page.waitForFunction(()=>window.__test?.ready)}}
async function editor(){
  await page.waitForFunction(()=>document.querySelector('.markdown-note .vditor-ir .vditor-reset[contenteditable="true"],.markdown-note .md-reading-body'))
  if(await page.locator('.md-reading-body').count())await page.getByRole('button',{name:'编辑正文',exact:true}).click()
  const input=page.locator('.markdown-note .vditor-ir .vditor-reset[contenteditable="true"]');await input.waitFor();return input
}
async function goToList(){await page.evaluate(()=>window.__test.reader.showList());await page.locator('.paper-list').waitFor()}

test('blank-area creation stores a Markdown note in the selected folder and opens the centre editor',async()=>{
  await open();await page.locator('.paper-list').click({button:'right',position:{x:600,y:550},timeout:5000})
  await page.getByRole('button',{name:'新建 Markdown 笔记',exact:true}).click()
  await editor();assert.equal(await page.locator('.markdown-note').count(),1)
  assert.match(await page.locator('.note-header .toolbar-title').getAttribute('title'),/读书笔记/)
  assert.equal(await page.locator('.note-header .color-picker .color-dot').count(),6)
  assert.equal(await page.getByRole('button',{name:'← 返回列表'}).count(),0)
  await page.locator('.note-editor').hover()
  assert.equal(await page.locator('.markdown-note .vditor-toolbar').isVisible(),false)
  const call=await page.evaluate(()=>window.__test.calls.find(c=>c.name==='create_library_note'))
  assert.equal(call.args.collectionId,'leaf');assert.equal(call.args.expectedRoot,'/mock-library')
  await page.getByRole('textbox',{name:'笔记标题',exact:true}).fill('我的读书笔记')
  await (await editor()).fill('第一段阅读记录与公式 $x^2$。')
  await page.getByRole('button',{name:'保存',exact:true}).click()
  await page.waitForFunction(()=>[...window.__test.notes.values()][0].content.includes('第一段阅读记录'))
  await goToList()
  await page.locator('.paper-row').filter({hasText:'我的读书笔记'}).dblclick()
  await page.locator('.md-reading-body').waitFor()
  assert.match(await page.locator('.md-reading-body').textContent(),/第一段阅读记录/)
  assert.match(await (await editor()).textContent(),/第一段阅读记录/)
  assert.equal(await page.getByRole('textbox',{name:'笔记标题',exact:true}).inputValue(),'我的读书笔记')
  await page.evaluate(()=>new Promise(requestAnimationFrame))
  await page.screenshot({path:'/private/tmp/argus-library-note-editor.png'})
})
test('the import dropdown creates another independent note',async()=>{
  await goToList()
  await page.locator('.import-wrap > button').click()
  await page.getByRole('button',{name:'新建笔记',exact:true}).click()
  await editor();assert.equal(await page.evaluate(()=>window.__test.notes.size),2)
  assert.notEqual(await page.evaluate(()=>window.__test.reader.activeSlug),'note_1')
})
test('an AI/manual save conflict retains the draft and refuses a stale overwrite',async()=>{
  await page.evaluate(()=>window.__test.reader.openPaper('note_1','我的读书笔记','md'));await editor()
  await (await editor()).fill('尚未保存的用户草稿')
  await page.evaluate(()=>window.__test.external('AI 已批准写入的新版本'))
  await page.getByRole('button',{name:'保存',exact:true}).click()
  await page.getByText('Note changed; draft retained',{exact:false}).waitFor()
  assert.match(await (await editor()).textContent(),/尚未保存的用户草稿/)
  assert.equal(await page.evaluate(()=>window.__test.notes.get('note_1').content),'AI 已批准写入的新版本')
})
test('reopening a conflicted draft never silently rebases it over the approved version',async()=>{
  for(let i=0;i<2;i++){
    await page.evaluate(()=>{window.__test.reader.closeTab('note_1');window.__test.reader.showList()})
    await page.locator('.paper-row').filter({hasText:'我的读书笔记'}).dblclick()
    await editor()
    await page.getByText('笔记已在别处更新，当前草稿已保留。请核对后重新载入。',{exact:false}).waitFor()
    assert.match(await (await editor()).textContent(),/用户草稿/)
    await (await editor()).fill('反复关闭后仍保留的用户草稿')
    await page.getByRole('button',{name:'保存',exact:true}).click()
    assert.equal(await page.evaluate(()=>window.__test.notes.get('note_1').content),'AI 已批准写入的新版本')
  }
  await page.getByRole('button',{name:'重新载入',exact:true}).click()
  await page.waitForFunction(()=>document.querySelector('.vditor-ir [contenteditable="true"]')?.textContent.includes('AI 已批准写入的新版本'))
})
test('typing during an in-flight save drains the latest draft in order',async()=>{
  await page.evaluate(()=>window.__test.holdSave=true)
  await (await editor()).fill('保存中的第一版')
  await page.getByRole('button',{name:'保存',exact:true}).click()
  await page.waitForFunction(()=>typeof window.__test.releaseSave==='function')
  await (await editor()).fill('保存期间继续输入的第二版')
  await page.evaluate(()=>{window.__test.holdSave=false;window.__test.releaseSave()})
  await page.waitForFunction(()=>window.__test.notes.get('note_1').content.includes('保存期间继续输入的第二版'))
  await page.locator('.save-note[title="已保存"]').waitFor()
})
test('all standalone-note approval cards identify the operation, show content and require a click',async()=>{
  for(const [tool,title,button] of [
    ['read_library_note','AI 想读取这篇笔记','允许读取'],['create_library_note','AI 想新建独立笔记','确认新建'],
    ['update_library_note','AI 想修改这篇笔记','确认修改'],['delete_library_note','AI 想删除这篇笔记','删除笔记'],
  ]){
    await page.evaluate(tool=>window.__test.preview(tool),tool)
    await page.getByText(title,{exact:true}).waitFor()
    assert.match(await page.locator('.wc-body').textContent(),/第二版|修改后的正文/)
    const before=await page.evaluate(()=>window.__test.approvals)
    assert.equal(await page.evaluate(()=>window.__test.approvals),before)
    await page.locator('.approval').getByRole('button',{name:button,exact:true}).click()
    assert.equal(await page.evaluate(()=>window.__test.approvals),before+1)
  }
  await page.evaluate(()=>window.__test.preview('read_library_note'))
  const before=await page.evaluate(()=>window.__test.approvals)
  await page.locator('.approval').getByRole('button',{name:'取消',exact:true}).click()
  assert.equal(await page.evaluate(()=>window.__test.approvals),before)
  assert.equal(await page.evaluate(()=>window.__test.rejections),1)
  assert.deepEqual(errors,[])
})

async function selectReadingText(text,selector='.md-reading-body') {
  const points=await page.evaluate(async({text,selector})=>{
    const {markdownTextIndex,markdownRange}=await import('/src/utils/markdownAnchors.ts')
    const root=document.querySelector(selector),index=markdownTextIndex(root),at=index.text.indexOf(text)
    if(at<0)throw new Error('No rendered excerpt: '+text)
    const range=markdownRange(index,at,at+text.length)
    const rects=[...range.getClientRects()],first=rects[0],last=rects.at(-1)
    return {start:{x:first.left+0.1,y:first.top+first.height/2},end:{x:last.right-0.1,y:last.top+last.height/2}}
  },{text,selector})
  await page.mouse.move(points.start.x,points.start.y);await page.mouse.down();await page.mouse.move(points.end.x,points.end.y,{steps:20});await page.mouse.up()
  await page.locator('.md-selection').waitFor()
}
test('Markdown reading selections persist annotations and the ordinary sidebar can edit and jump to them',async()=>{
  await (await editor()).fill('# 第一章\n\n可以批注的正文，保存以后还能找到。\n\n## 第二章\n\n另一段内容。')
  await page.getByRole('button',{name:'完成编辑',exact:true}).click()
  await page.locator('.md-reading-body h1').waitFor()
  await selectReadingText('可以批注的正文')
  await page.locator('.md-selection').getByRole('button',{name:'批注',exact:true}).click()
  await page.locator('.md-annotation-editor textarea').fill('这里是我的理解与公式 $x^2$。')
  await page.locator('.md-annotation-editor').getByRole('button',{name:'保存',exact:true}).click()
  await page.waitForFunction(()=>window.__test.highlights.get('note_1')?.[0]?.note?.includes('我的理解'))
  await page.locator('.md-mark').first().waitFor()
  await page.evaluate(()=>window.__test.setSidebar('highlights'))
  await page.locator('.highlights-tab .hl-note').filter({hasText:'这里是我的理解'}).waitFor()
  await page.locator('.highlights-tab').getByRole('button',{name:'跳转',exact:true}).click()
  await page.locator('.md-annotation-editor textarea').waitFor()
  await page.locator('.md-annotation-editor').getByRole('button',{name:'取消',exact:true}).click()
  await page.getByRole('button',{name:'编辑正文',exact:true}).click()
  await (await editor()).fill('在前面补充说明。\n\n# 第一章\n\n可以批注的正文，保存以后还能找到。\n\n## 第二章\n\n另一段内容。')
  await page.getByRole('button',{name:'完成编辑',exact:true}).click()
  await page.locator('.md-mark').first().waitFor()
  assert.equal(await page.locator('.md-anchor-warning').count(),0)
  const body=await page.evaluate(()=>window.__test.notes.get('note_1').content)
  assert(!body.includes('<mark'));assert(!body.includes('我的理解'))
  await page.screenshot({path:'/private/tmp/argus-markdown-annotations.png'})
  await page.evaluate(()=>{window.__test.reader.closeTab('note_1');window.__test.reader.showList()})
  await page.locator('.paper-row').filter({hasText:'我的读书笔记'}).dblclick()
  await page.locator('.md-reading-body').waitFor()
  await page.locator('.md-mark').first().waitFor()
  assert.match(await page.locator('.highlights-tab .hl-note').textContent(),/我的理解/)
})
test('editing selections use the same annotations and toolbar, and survive save, zoom, search and reopening',async()=>{
  await editor()
  const source=await page.evaluate(()=>window.__test.notes.get('note_1').content)
  await selectReadingText('另一段内容','.markdown-note .vditor-ir .vditor-reset')
  await page.locator('.note-header .color-picker .color-dot').nth(2).click()
  await page.waitForFunction(()=>window.__test.highlights.get('note_1')?.some(h=>h.text==='另一段内容'&&h.color==='#90CAF9'))
  const id=await page.evaluate(()=>window.__test.highlights.get('note_1').find(h=>h.text==='另一段内容').id)
  await page.locator(`.md-editor-overlay .md-mark[data-hl-id="${id}"]`).first().click()
  await page.locator('.md-annotation-editor textarea').fill('编辑时也能直接批注。')
  await page.locator('.md-annotation-editor').getByRole('button',{name:'保存',exact:true}).click()
  await page.locator('.font-controls button').last().click()
  await page.locator('.md-editor-overlay .md-mark').first().waitFor()
  await page.getByRole('button',{name:'完成编辑',exact:true}).click()
  await page.locator('.md-reading-body').waitFor()
  await page.locator(`.md-mark[data-hl-id="${id}"]`).first().waitFor()
  await page.locator('.note-header .icon-btn').click()
  await page.locator('.md-note-find input').fill('另一段内容')
  await page.locator('.md-note-find').filter({hasText:'1 / 1'}).waitFor()
  await page.locator('.md-note-find input').press('Escape')
  assert.equal(await page.evaluate(()=>window.__test.notes.get('note_1').content),source,'annotations must not enter saved Markdown')
  await page.evaluate(()=>{window.__test.reader.closeTab('note_1');window.__test.reader.showList()})
  await page.locator('.paper-row').filter({hasText:'我的读书笔记'}).dblclick()
  await page.locator('.md-reading-body').waitFor()
  await page.locator(`.md-mark[data-hl-id="${id}"]`).first().click()
  assert.equal(await page.locator('.md-annotation-editor textarea').inputValue(),'编辑时也能直接批注。')
  await page.locator('.md-annotation-editor').getByRole('button',{name:'删除',exact:true}).click()
  await page.waitForFunction(id=>!window.__test.highlights.get('note_1').some(h=>h.id===id),id)
  await page.waitForFunction(id=>!document.querySelector(`.md-mark[data-hl-id="${id}"]`),id)
  assert.equal(await page.locator(`.md-mark[data-hl-id="${id}"]`).count(),0)
})
test('formatted editable selections ignore syntax markers and annotations track scrolling and text reflow',async()=>{
  const input=await editor()
  await input.fill('# 第一章\n\n可以批注的正文，保存以后还能找到。\n\n## 第二章\n\n另一段内容。\n\n包含**加粗文字**和普通文字。\n\n```python\nresult = score * 2\n```\n\n'+Array.from({length:45},(_,i)=>`滚动测试第${i+1}段文字。`).join('\n\n'))
  await page.getByRole('button',{name:'完成编辑',exact:true}).click()
  await page.locator('.md-reading-body strong').waitFor()
  await editor()
  await selectReadingText('包含加粗文字和普通文字。','.markdown-note .vditor-ir .vditor-reset')
  await page.locator('.md-selection .md-mark-colors button').nth(1).click()
  await page.waitForFunction(()=>window.__test.highlights.get('note_1').some(h=>h.text==='包含加粗文字和普通文字。'&&h.color==='#A5D6A7'))
  const id=await page.evaluate(()=>window.__test.highlights.get('note_1').find(h=>h.text==='包含加粗文字和普通文字。').id)
  await selectReadingText('result = score * 2','.markdown-note .vditor-ir .vditor-reset')
  await page.locator('.md-selection').getByRole('button',{name:'批注',exact:true}).click()
  await page.locator('.md-annotation-editor textarea').fill('代码段也能批注。')
  await page.locator('.md-annotation-editor').getByRole('button',{name:'保存',exact:true}).click()
  await page.waitForFunction(()=>window.__test.highlights.get('note_1').some(h=>h.text==='result = score * 2'))
  const offset=()=>page.evaluate(async id=>{
    const {markdownTextIndex,markdownRange}=await import('/src/utils/markdownAnchors.ts')
    const root=document.querySelector('.markdown-note .vditor-ir .vditor-reset'),index=markdownTextIndex(root),at=index.text.indexOf('包含加粗文字和普通文字。')
    const text=markdownRange(index,at,at+1).getBoundingClientRect(),mark=document.querySelector(`.md-mark[data-hl-id="${id}"]`).getBoundingClientRect()
    return {x:Math.abs(text.left-mark.left),y:Math.abs(text.top-mark.top)}
  },id)
  await page.waitForFunction(id=>!!document.querySelector(`.md-mark[data-hl-id="${id}"]`),id)
  let diff=await offset();assert(diff.x<2&&diff.y<2)
  await page.evaluate(()=>document.querySelector('.markdown-note .vditor-ir .vditor-reset').scrollTop=90)
  assert(await page.evaluate(()=>document.querySelector('.markdown-note .vditor-ir .vditor-reset').scrollTop>=90),'the editor must actually scroll during the geometry check')
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))))
  diff=await offset();assert(diff.x<2&&diff.y<2)
  await page.locator('.font-controls button').last().click()
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))))
  diff=await offset();assert(diff.x<2&&diff.y<2)
  await page.locator('.note-header .icon-btn').click();await page.locator('.md-note-find input').fill('滚动测试第45段文字。')
  await page.locator('.md-note-find').filter({hasText:'1 / 1'}).waitFor()
  await page.waitForFunction(()=>{
    const root=document.querySelector('.markdown-note .vditor-ir .vditor-reset'),text=[...root.querySelectorAll('p')].find(p=>p.textContent.includes('滚动测试第45段文字。'))
    if(!text)return false
    const box=root.getBoundingClientRect(),hit=text.getBoundingClientRect();return hit.top>=box.top&&hit.bottom<=box.bottom
  })
  await page.locator('.md-note-find input').press('Escape')
  await page.getByRole('button',{name:'完成编辑',exact:true}).click()
  await page.locator(`.md-mark[data-hl-id="${id}"]`).first().waitFor()
  const body=await page.evaluate(()=>window.__test.notes.get('note_1').content)
  assert.match(body,/\*\*加粗文字\*\*/);assert(!body.includes('data-hl-id'))
})
test('standalone documents can own separate notes without changing their primary Markdown body',async()=>{
  const original=await page.evaluate(()=>window.__test.notes.get('note_1').content)
  await page.evaluate(()=>window.__test.setSidebar('notes'))
  await page.locator('.notes-tab .new-btn').click()
  const attached=page.locator('.notes-tab .vditor-ir [contenteditable="true"]')
  await attached.waitFor();await attached.fill('这是独立于正文的附属笔记。')
  await page.waitForFunction(()=>window.__test.attachments.get('note_1')?.[0]?.content?.includes('附属笔记'))
  assert.equal(await page.evaluate(()=>window.__test.notes.get('note_1').content),original)
  await page.evaluate(()=>window.__test.setSidebar('highlights'))
  await page.evaluate(()=>window.__test.setSidebar('notes'))
  await attached.waitFor();assert.match(await attached.textContent(),/附属笔记/)
})
test('the real AI tab sends document-scoped requests and keeps separate histories per Markdown document',async()=>{
  await page.evaluate(()=>window.__test.setSidebar('ai'))
  const composer=page.locator('.test-sidebar .composer-input')
  await composer.waitFor();await composer.fill('请解释这篇笔记。');await composer.press('Enter')
  const alignment=await page.evaluate(()=>{const note=document.querySelector('.note-header').getBoundingClientRect(),ai=document.querySelector('.ai-header').getBoundingClientRect();return {noteTop:note.top,aiTop:ai.top,noteHeight:note.height,aiHeight:ai.height}})
  assert.equal(alignment.noteTop,alignment.aiTop);assert.equal(alignment.noteHeight,alignment.aiHeight)
  await page.screenshot({path:'/private/tmp/argus-markdown-header-aligned.png'})
  await page.waitForFunction(()=>window.__test.calls.some(c=>c.name==='chat_with_library'))
  const call=await page.evaluate(()=>window.__test.calls.find(c=>c.name==='chat_with_library'))
  assert.equal(call.args.paperSlug,'note_1');assert.equal(call.args.expectedRoot,'/mock-library')
  await page.getByText('这是当前独立笔记的模拟 AI 回复。',{exact:true}).waitFor()
  await page.evaluate(()=>window.__test.reader.openPaper('note_2','另一篇笔记','md'))
  await composer.waitFor();assert.equal(await page.locator('.test-sidebar .user-bubble').filter({hasText:'请解释这篇笔记'}).count(),0)
  await page.evaluate(()=>window.__test.reader.openPaper('note_1','我的读书笔记','md'))
  await page.locator('.test-sidebar .user-bubble').filter({hasText:'请解释这篇笔记'}).waitFor()
  await page.evaluate(()=>window.__test.setSidebar('sections'))
  await page.locator('.sections-tab .section-item').filter({hasText:'第二章'}).click()
  await page.locator('.md-reading-body h2').waitFor()
  assert(!await page.evaluate(()=>window.__test.calls.some(c=>c.name==='regen_ebook_sections')))
  assert.deepEqual(errors,[])
})
test('Markdown anchors exclude hidden math, survive Unicode text shifts and refuse ambiguous matches',async()=>{
  const result=await page.evaluate(async()=>{
    const {markdownTextIndex,markdownSelection,locateMarkdownQuote}=await import('/src/utils/markdownAnchors.ts')
    const root=document.createElement('article');root.innerHTML='<p>😀前文<strong>重点文字</strong>后文</p><span class="katex-mathml">hidden TeX</span><button>copy</button>'
    const index=markdownTextIndex(root),node=root.querySelector('strong').firstChild,range=document.createRange();range.setStart(node,0);range.setEnd(node,4)
    const selection=markdownSelection(root,range)
    const relocated=locateMarkdownQuote('添加开头。'+index.text,{text:selection.text,start_offset:selection.start,end_offset:selection.end,anchor_prefix:selection.prefix,anchor_suffix:selection.suffix})
    return {text:index.text,selection,relocated,ambiguous:locateMarkdownQuote('重复原文，重复原文',{text:'重复原文'})}
  })
  assert.equal(result.text,'😀前文重点文字后文')
  assert.equal(result.selection.start,4)
  assert.equal(result.relocated.start,9)
  assert.equal(result.ambiguous,null)
})

test('real stores: multiple notes appear and open immediately while an older library scan is still blocked',async()=>{
  await open(true)
  await page.evaluate(()=>{const t=window.__test;t.holdScans=true;t.oldScan=t.library.refresh()})
  await page.waitForFunction(()=>window.__test.pendingScans.length===1)
  await page.locator('.paper-list').click({button:'right',position:{x:600,y:550}})
  await page.getByRole('button',{name:'新建 Markdown 笔记',exact:true}).click()
  await editor()
  await goToList()
  await page.locator('.paper-row').filter({hasText:'新建笔记'}).waitFor()
  await page.locator('.paper-list').click({button:'right',position:{x:600,y:550}})
  const create=page.getByRole('button',{name:'新建 Markdown 笔记',exact:true})
  assert.equal(await create.isEnabled(),true,'a completed creation must not stay disabled while a scan is pending')
  await create.click();await editor()
  await page.locator('.import-wrap > button').click()
  const menuCreate=page.getByRole('button',{name:'新建笔记',exact:true})
  assert.equal(await menuCreate.isEnabled(),true)
  await menuCreate.click();await editor()
  assert.equal(await page.evaluate(()=>window.__test.notes.size),3)
  assert.deepEqual(await page.evaluate(()=>window.__test.library.papers.map(n=>n.title)),['新建笔记','新建笔记 2','新建笔记 3'])
  assert.equal(await page.evaluate(()=>window.__test.collections.paperIdsInCollection('leaf').length),3)
  assert.equal(await page.evaluate(()=>window.__test.reader.activeSlug),'note_3')
  await page.evaluate(async()=>{const t=window.__test;t.holdScans=false;t.pendingScans.shift()();await t.oldScan})
  assert.equal(await page.evaluate(()=>window.__test.reader.activeSlug),'note_3','the old empty scan must not prune the newly opened note')
  assert.equal(await page.evaluate(()=>window.__test.reader.tabs.length),3)
  await goToList()
  assert.equal(await page.locator('.paper-row').count(),3)
})
test('real stores: a failed creation does not add a row or prevent a subsequent retry',async()=>{
  await open(true);await page.evaluate(()=>window.__test.failCreate=true)
  await page.locator('.import-wrap > button').click();await page.getByRole('button',{name:'新建笔记',exact:true}).click()
  await page.getByText('Mock creation failed',{exact:false}).waitFor()
  assert.equal(await page.evaluate(()=>window.__test.library.papers.length),0)
  await page.evaluate(()=>window.__test.failCreate=false)
  await page.locator('.import-wrap > button').click()
  const create=page.getByRole('button',{name:'新建笔记',exact:true});assert.equal(await create.isEnabled(),true)
  await create.click();await editor()
  assert.equal(await page.evaluate(()=>window.__test.notes.size),1)
  assert.deepEqual(errors,[])
})
