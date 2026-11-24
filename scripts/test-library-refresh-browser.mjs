// Real library/collection stores, list and virtual scroller. Local mocked disk only.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  // The sidebar keeps chat mounted in the background; metadata tests do not
  // need a model session. List, metadata panel, sidebar and tab bar remain real.
  './tabs/AnalysisTab.vue': `export default {render:()=>null};`,
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args);export const convertFileSrc=p=>p;export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,handler)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(handler);return()=>set.delete(handler)};export const emit=async(name,payload)=>window.__test.emit(name,payload);export const emitTo=emit;`,
  '@tauri-apps/api/window': `export const getCurrentWindow=()=>({label:'main'});export class Window{static async getByLabel(){return null}}`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow=()=>({label:'main',isFullscreen:async()=>false,onResized:async()=>()=>{}});export class WebviewWindow{static async getByLabel(){return null}}`,
  '@tauri-apps/plugin-dialog': `export const confirm=async()=>false;export const save=async()=>null;export const open=async()=>null;`,
}
before(async()=>{
  server=await createServer({configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',cacheDir:'node_modules/.vite-library-refresh-test',
    plugins:[{name:'refresh-io',enforce:'pre',resolveId:s=>s in mocks?'\0refresh-mock:'+s:null,load:id=>id.startsWith('\0refresh-mock:')?mocks[id.slice(14)]:null},vue()],
    optimizeDeps:{entries:['src/components/PaperList.vue']},server:{host:'127.0.0.1',port:0},logLevel:'error'})
  server.middlewares.use(async(req,res,next)=>{
    if(!req.url.startsWith('/__refresh-test'))return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html,body,#app{margin:0;height:100%;overflow:hidden}.paper-list{height:100%}.test-workspace{height:100%;display:flex;flex-direction:column}.test-main{flex:1;min-height:0;display:flex}.test-main>.paper-list{flex:1;min-width:0}.test-main>.right-sidebar{width:360px;flex-shrink:0}</style><script type="module">
      import{createApp,h,nextTick}from'vue';import{createPinia}from'pinia';import{addCollection}from'@iconify/vue';import fluent from'@iconify-json/fluent/icons.json';
      import PaperList from'/src/components/PaperList.vue';import{useLibraryStore}from'/src/stores/library.ts';import{useCollectionsStore}from'/src/stores/collections.ts';import{useSelectionStore}from'/src/stores/selection.ts';
      import{useReaderStore}from'/src/stores/reader.ts';
      const metadataMode=new URLSearchParams(location.search).has('metadata');
      const RightSidebar=metadataMode?(await import('/src/components/RightSidebar.vue')).default:null;
      const TabBar=metadataMode?(await import('/src/components/TabBar.vue')).default:null;
      import{i18n}from'/src/i18n/index.ts';import'/src/assets/main.css';import'vue-virtual-scroller/dist/vue-virtual-scroller.css';addCollection(fluent);
      const clone=x=>JSON.parse(JSON.stringify(x));
      const row=i=>({slug:'paper-'+i,id:'id-'+i,title:'Research paper '+i,authors:['Author '+i],tags:[],added_at:new Date(2026,0,i+1).toISOString(),reading_status:'unread',year:2026,has_bibtex:false,meta_mtime:1,status:{text_extracted:true,metadata_fetched:true,ai_summary_done:false,vectorized:false,last_updated:'2026-01-01'}});
      const polls=[],realInterval=window.setInterval.bind(window);
      window.setInterval=(fn,ms,...args)=>{if(ms===60000){polls.push(fn);return realInterval(()=>{},ms)}return realInterval(fn,ms,...args)};
      const t=window.__test={root:'/mock-library',disk:Array.from({length:12},(_,i)=>row(i)),row,listeners:new Map(),calls:[],hold:false,releases:[],metas:new Map(),holdSave:false,failSave:false,
        collectionFile:{collections:[{id:'top',name:'图书'},{id:'current',name:'阅读',parent_id:'top'},{id:'other',name:'其他',parent_id:'top'}],assignments:Array.from({length:12},(_,i)=>({paper_id:'id-'+i,collection_id:i<8?'current':'other'}))},
        emit(name,payload){for(const fn of t.listeners.get(name)||[])fn({payload})},
        async invoke(name,args){t.calls.push({name,args});
          if(name==='open_library')return {root:args.root};
          if(name==='load_library_cache')return clone(t.disk);
          if(name==='scan_library'){const snapshot=clone(t.disk);if(t.hold)return new Promise(resolve=>t.releases.push(()=>resolve(snapshot)));return snapshot}
          if(name==='get_collections')return clone(t.collectionFile);
          if(name==='get_canvas_note_titles_map')return {};
          if(name==='get_current_library')return t.root;
          if(name==='get_activity_log')return {version:2,devices:{},days:{}};
          if(name==='get_paper_meta')return clone(t.metas.get(args.slug));
          if(name==='save_paper_meta'){
            const saved=clone(args.meta);if(t.holdSave)await new Promise(resolve=>t.releaseSave=resolve);
            if(t.failSave)throw new Error('Mock metadata save failed');
            t.metas.set(args.slug,saved);const entry=t.disk.find(p=>p.slug===args.slug);Object.assign(entry,{title:saved.title,authors:saved.authors,tags:saved.tags,year:saved.year,venue:saved.venue,reading_status:saved.reading_status,has_bibtex:!!saved.bibtex?.trim()});return null;
          }
          if(name==='get_cached_references'||name==='fetch_references')return [];
          if(name==='get_fulltext')return '';
          if(name==='get_settings')return {};
          if(name==='get_venue_ranks')return {};
          if(name==='copy_paper_pdfs')return args.slugs.length;
          return null;
        }};
      t.disk.forEach(p=>t.metas.set(p.slug,{...clone(p),venue:'Test Venue',doi:'10.1234/mock',cite_count:1,bibtex:'',related_ids:[]}));
      const pinia=createPinia(),app=createApp({render:()=>metadataMode?h('div',{class:'test-workspace'},[h(TabBar),h('div',{class:'test-main'},[h(PaperList),h(RightSidebar,{activeTab:'metadata'})])]):h(PaperList)});app.use(pinia);app.use(i18n);
      t.library=useLibraryStore(pinia);t.collections=useCollectionsStore(pinia);t.selection=useSelectionStore(pinia);t.reader=useReaderStore(pinia);
      await t.library.openLibrary(t.root);await t.collections.load();t.selection.selectNav('collection:current');if(metadataMode)t.reader.openPaper('paper-7','Research paper 7');app.mount('#app');
      t.settle=async()=>{await nextTick();await new Promise(resolve=>setTimeout(resolve,0));await new Promise(requestAnimationFrame)};
      t.poll=async()=>{for(const fn of polls)fn();await t.settle()};
      t.observe=()=>{
        const root=document.querySelector('.vue-recycle-scroller__item-wrapper');t.rows=[...document.querySelectorAll('.paper-row')];
        t.rowData=t.rows.map(el=>({el,text:el.querySelector('.row-title').textContent}));t.list=t.library.papers;t.objects=[...t.list];t.mutations=[];
        t.observer=new MutationObserver(records=>t.mutations.push(...records.map(r=>({type:r.type,attribute:r.attributeName}))));t.observer.observe(root,{subtree:true,childList:true,characterData:true,attributes:true});
      };
      t.inspect=()=>({sameList:t.list===t.library.papers,sameObjects:t.objects.every((p,i)=>p===t.library.papers[i]),
        sameRows:t.rowData.every(({el,text})=>el.isConnected&&el.querySelector('.row-title').textContent===text),mutations:t.mutations.length,
        selected:[...t.selection.selectedSlugs],scroll:document.querySelector('.vue-recycle-scroller').scrollTop});
      t.ready=true;
      </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true});page=await browser.newPage({viewport:{width:1200,height:700}})
  page.setDefaultTimeout(10000);page.on('pageerror',e=>{errors.push(e.message);console.error(e.message)})
  await page.route('**/*',r=>r.request().url().startsWith(origin)?r.continue():r.abort())
})
after(async()=>{await browser?.close();await server?.close()})
async function open(metadata=false){await page.goto(origin+'/__refresh-test'+(metadata?'?metadata=1':''));await page.waitForFunction(()=>window.__test?.ready).catch(async e=>{console.log(await page.evaluate(()=>({text:document.body.innerText,test:window.__test?{calls:window.__test.calls,root:window.__test.library?.currentPath,error:window.__test.library?.error}:null})));throw e});await page.locator('.paper-row').first().waitFor();await page.evaluate(()=>window.__test.settle())}

test('idle reconciliation keeps unchanged rows, selection and DOM untouched',async()=>{
  await open()
  const result=await page.evaluate(async()=>{
    const t=window.__test;t.selection.selectRange(['paper-7','paper-6'],'paper-7');await t.settle();t.observe()
    for(let i=0;i<12;i++)await t.poll()
    const result=t.inspect();t.observer.disconnect();return result
  })
  assert.equal(result.sameList,true,'an unchanged disk scan must retain the list')
  assert.equal(result.sameObjects,true);assert.equal(result.sameRows,true,'virtual rows must not be reassigned on a no-op refresh')
  assert.equal(result.mutations,0,'idle scans must not repaint the visible list')
  assert.deepEqual(result.selected,['paper-7','paper-6'])
})
test('real metadata updates are applied in place and changes elsewhere do not recycle the current collection',async()=>{
  await open()
  const result=await page.evaluate(async()=>{
    const t=window.__test;t.observe();t.disk[10].title='Updated outside this folder';await t.library.refresh();await t.settle()
    const elsewhere=t.inspect();t.observer.disconnect()
    const object=t.library.papers.find(p=>p.slug==='paper-7');t.disk[7].title='Updated current title';t.disk[7].status.ai_summary_done=true;t.disk[7].tags=['New tag'];delete t.disk[7].year
    await t.library.refresh();await t.settle()
    return {elsewhere,sameObject:object===t.library.papers.find(p=>p.slug==='paper-7'),title:object.title,year:object.year,aiDone:object.status.ai_summary_done,tags:object.tags,text:document.querySelector('.paper-list').textContent}
  })
  assert.equal(result.elsewhere.mutations,0);assert.equal(result.elsewhere.sameRows,true)
  assert.equal(result.sameObject,true);assert.equal(result.title,'Updated current title');assert.equal(result.year,undefined)
  assert.equal(result.aiDone,true);assert.deepEqual(result.tags,['New tag']);assert.match(result.text,/Updated Current Title/)
})
test('additions, deletions and slug changes still update membership and prune stale reader tabs',async()=>{
  await open()
  const result=await page.evaluate(async()=>{
    const t=window.__test;
    t.reader.openPaper('paper-7','Research paper 7');t.reader.openPaper('paper-6','Research paper 6');
    t.disk.push(t.row(20));t.collectionFile.assignments.push({paper_id:'id-20',collection_id:'current'});await t.collections.load();await t.library.refresh();await t.settle()
    const added=document.querySelector('.paper-list').textContent.includes('Research Paper 20')
    t.disk=t.disk.filter(p=>p.slug!=='paper-7');t.disk.find(p=>p.slug==='paper-6').slug='renamed-paper-6'
    await t.library.refresh();await t.settle()
    return {added,slugs:t.library.papers.map(p=>p.slug),tabs:t.reader.tabs.map(t=>t.slug),count:document.querySelector('.list-count').textContent}
  })
  assert.equal(result.added,true);assert(!result.slugs.includes('paper-7'));assert(!result.slugs.includes('paper-6'));assert(result.slugs.includes('renamed-paper-6'))
  assert.equal(result.count.trim(),'8')
  assert.deepEqual(result.tabs,[])
  assert.deepEqual(errors,[])
})
test('unchanged reconciliation preserves a scrolled virtual viewport in a large collection',async()=>{
  await open()
  const result=await page.evaluate(async()=>{
    const t=window.__test;t.disk=Array.from({length:300},(_,i)=>t.row(i));t.collectionFile.assignments=t.disk.map(p=>({paper_id:p.id,collection_id:'current'}))
    await t.collections.load();await t.library.refresh();await t.settle()
    const scroller=document.querySelector('.vue-recycle-scroller');scroller.scrollTop=1800;scroller.dispatchEvent(new Event('scroll'))
    await new Promise(resolve=>setTimeout(resolve,350));await t.settle();const before=scroller.scrollTop;t.observe()
    for(let i=0;i<12;i++)await t.poll()
    const snapshot=t.inspect();t.observer.disconnect();return {before,snapshot}
  })
  assert(result.before>0);assert.equal(result.snapshot.scroll,result.before)
  assert.equal(result.snapshot.sameRows,true);assert.equal(result.snapshot.mutations,0)
})
test('a late scan from the previous library cannot replace or prune the newly opened library',async()=>{
  await open()
  const result=await page.evaluate(async()=>{
    const t=window.__test;t.hold=true;const previous=t.library.refresh();await t.settle()
    const next={...t.row(50),slug:'new-library-paper',id:'new-library-id'}
    t.library.currentPath='/new-library';t.library.papers=[next];t.disk=[next];t.reader.openPaper(next.slug,next.title);t.hold=false
    const current=t.library.refresh();t.releases.shift()();await Promise.all([previous,current]);await t.settle()
    return {slugs:t.library.papers.map(p=>p.slug),tabs:t.reader.tabs.map(p=>p.slug),refreshing:t.library.isRefreshing}
  })
  assert.deepEqual(result.slugs,['new-library-paper']);assert.deepEqual(result.tabs,['new-library-paper']);assert.equal(result.refreshing,false)
  assert.deepEqual(errors,[])
})

test('saving the sidebar title updates the real list and tab before scans finish, and an older scan cannot revert it',async()=>{
  await open(true)
  const sidebar=page.locator('.meta-tab')
  await sidebar.locator('.action-bar').getByRole('button',{name:'编辑',exact:true}).click()
  await sidebar.locator('input').first().fill('Course-project-doc')
  await page.evaluate(async()=>{
    const t=window.__test;t.selection.selectPaper('paper-7');
    t.originalList=t.library.papers;t.originalRow=t.library.papers.find(p=>p.slug==='paper-7');
    t.originalRow.status.vectorized=true;t.originalRow.related_ids=['new-link'];
    t.hold=true;t.scanning=t.library.refresh();await t.settle()
  })
  await sidebar.locator('.action-bar').getByRole('button',{name:'保存',exact:true}).click()
  await sidebar.getByText('Course-project-doc',{exact:true}).waitFor()
  await page.locator('.row-title').filter({hasText:'Course-Project-Doc'}).waitFor()
  await page.locator('.tab-title').filter({hasText:'Course-Project-Doc'}).waitFor()
  const immediate=await page.evaluate(()=>{
    const t=window.__test,p=t.library.papers.find(p=>p.slug==='paper-7');
    return {sameList:t.originalList===t.library.papers,sameRow:t.originalRow===p,vectorized:p.status.vectorized,links:p.related_ids,selected:t.selection.selectedSlug,scans:t.releases.length}
  })
  assert.equal(immediate.sameList,true);assert.equal(immediate.sameRow,true);assert.equal(immediate.vectorized,true)
  assert.deepEqual(immediate.links,['new-link']);assert.equal(immediate.selected,'paper-7');assert.equal(immediate.scans,1)
  await page.evaluate(async()=>{const t=window.__test;t.releases.shift()();await t.settle()})
  assert.equal(await page.evaluate(()=>window.__test.library.papers.find(p=>p.slug==='paper-7').title),'Course-project-doc')
  assert.equal(await page.locator('.tab-paper .tab-title').textContent(),'Course-Project-Doc')
  await page.evaluate(async()=>{const t=window.__test;t.hold=false;t.releases.shift()();await t.scanning;await t.settle()})
  assert.equal(await page.locator('.row-title').filter({hasText:'Course-Project-Doc'}).count(),1)
  assert.deepEqual(errors,[])
})

test('pending and failed metadata saves keep the list and tab on the saved title',async()=>{
  await open(true)
  const sidebar=page.locator('.meta-tab')
  await sidebar.locator('.action-bar').getByRole('button',{name:'编辑',exact:true}).click()
  await sidebar.locator('input').first().fill('Unsaved title')
  await page.evaluate(()=>{const t=window.__test;t.holdSave=true;t.failSave=true})
  await sidebar.locator('.action-bar').getByRole('button',{name:'保存',exact:true}).click()
  await page.waitForFunction(()=>!!window.__test.releaseSave)
  assert.equal(await page.locator('.tab-paper .tab-title').textContent(),'Research Paper 7')
  assert.equal(await page.locator('.row-title').filter({hasText:'Unsaved Title'}).count(),0)
  await page.evaluate(()=>window.__test.releaseSave())
  await sidebar.getByRole('alert').filter({hasText:'Mock metadata save failed'}).waitFor()
  assert.equal(await page.locator('.tab-paper .tab-title').textContent(),'Research Paper 7')
  assert.equal(await sidebar.locator('input').first().inputValue(),'Unsaved title')
  assert.equal(await page.evaluate(()=>window.__test.library.papers.find(p=>p.slug==='paper-7').title),'Research paper 7')
  assert.deepEqual(errors,[])
})

test('real multi-selection and context-menu clicks copy every selected PDF in one request',async()=>{
  await open()
  const rows=page.locator('.paper-row')
  await rows.nth(0).click()
  await rows.nth(2).click({modifiers:['Meta']})
  await rows.nth(4).click({modifiers:['Meta']})
  assert.equal(await page.locator('.paper-row.selected').count(),3)
  await rows.nth(2).click({button:'right'})
  await page.getByRole('button',{name:'复制 PDF 文件（3）',exact:true}).click()
  const copied=await page.evaluate(()=>window.__test.calls.filter(c=>c.name==='copy_paper_pdfs'))
  assert.deepEqual(copied,[{name:'copy_paper_pdfs',args:{slugs:['paper-7','paper-5','paper-3'],expectedRoot:'/mock-library'}}])
  assert.equal(await page.locator('.paper-row.selected').count(),3)
  assert.deepEqual(errors,[])
})

test('clicking list whitespace clears all selected rows and resets the Shift-selection anchor',async()=>{
  await open()
  const rows=page.locator('.paper-row')
  await rows.nth(0).click()
  await rows.nth(2).click({modifiers:['Shift']})
  assert.equal(await page.locator('.paper-row.selected').count(),3)
  await rows.nth(1).click({button:'right'})
  await page.getByRole('button',{name:'复制 PDF 文件（3）',exact:true}).waitFor()
  const scroller=page.locator('.scroller'),box=await scroller.boundingBox(),last=await rows.last().boundingBox()
  assert(box.y+box.height-20>last.y+last.height,'click must land in the empty area below the rows')
  await scroller.click({position:{x:20,y:box.height-20}})
  assert.equal(await page.locator('.ctx-menu').count(),0)
  assert.equal(await page.locator('.paper-row.selected').count(),0)
  assert.deepEqual(await page.evaluate(()=>({slugs:window.__test.selection.selectedSlugs,primary:window.__test.selection.selectedSlug})),{slugs:[],primary:null})
  await rows.nth(2).click({modifiers:['Shift']})
  assert.equal(await page.locator('.paper-row.selected').count(),1,'a cleared selection must not reuse the previous range anchor')
  assert.deepEqual(errors,[])
})
