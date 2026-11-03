// The real viewer, render policy and PDF.js worker; mocked library IO only.
// Optional ARGUS_TEST_PDF exercises a local book without changing its library.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin, profile
const errors = []
function fixturePdf(seed = '') {
  const objects = ['<< /Type /Catalog /Pages 2 0 R >>', '', '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
  const kids = []
  for (let i = 0; i < 32; i++) {
    const p = objects.length + 1, text = `0.1 0.2 0.6 rg 20 20 360 560 re f BT /F1 24 Tf 40 300 Td (Page ${i + 1} ${seed}) Tj ET`
    kids.push(`${p} 0 R`)
    objects.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /Resources << /Font << /F1 3 0 R >> >> /Contents ${p + 1} 0 R >>`, `<< /Length ${text.length} >>\nstream\n${text}\nendstream`)
  }
  objects[1] = `<< /Type /Pages /Count ${kids.length} /Kids [${kids.join(' ')}] >>`
  let pdf = '%PDF-1.5\n'; const offsets = [0]
  objects.forEach((obj, i) => { offsets.push(Buffer.byteLength(pdf)); pdf += `${i + 1} 0 obj\n${obj}\nendobj\n` })
  const start = Buffer.byteLength(pdf)
  return Buffer.from(pdf + `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${offsets.slice(1).map(n => String(n).padStart(10, '0') + ' 00000 n \n').join('')}trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${start}\n%%EOF\n`)
}
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args);`,
  '../stores/reader': `export const useReaderStore=()=>window.__test.reader;`,
  '../stores/library': `export const useLibraryStore=()=>window.__test.library;`,
  '../stores/speech': `export const useSpeechStore=()=>window.__test.speech;`,
  '../stores/translationHistory': `export const runTranslation=()=>{};export const triggerAskAi=()=>{};`,
  '../stores/snippetLibrary': `export const openAddSnippetModal=()=>{};`,
}
before(async () => {
  server = await createServer({ configFile:false, root:new URL('../', import.meta.url).pathname, appType:'custom',
    plugins:[{ name:'viewer-test-io', enforce:'pre', resolveId(source, importer) {
      if (source in mocks && (source === '@tauri-apps/api/core' || importer?.endsWith('/src/components/PdfViewer.vue'))) return '\0viewer-mock:' + source
    }, load(id) { if (id.startsWith('\0viewer-mock:')) return mocks[id.slice(13)] },
    transform(code, id) {
      if (id.endsWith('/src/utils/pdfPageCache.ts?cache-limits-test')) return code.replace("'argus-pdf-pages-v1'", "'argus-pdf-pages-limits-test'").replace('512 * 1024 * 1024', '1024 * 1024').replace('MAX_ENTRIES = 400', 'MAX_ENTRIES = 3')
      if (!id.endsWith('/src/components/PdfViewer.vue')) return
      return code.replace('defineExpose({', `defineExpose({scale, requestRenderPage, snapshot:()=>({rendered:[...renderedPages.value],inFlight:[...startedRenders],queued:[...wantQueue],ops:[...opListPages],pixels:heldPixels(null),document:documentCacheKey}),`)
    } }, vue()], cacheDir:'node_modules/.vite-pdf-scroll-test',
    optimizeDeps:{ entries:['src/components/PdfViewer.vue'], include:['pdfjs-dist/legacy/build/pdf.mjs'] },
    server:{ host:'127.0.0.1', port:0 }, logLevel:'error' })
  server.middlewares.use(async (req, res, next) => {
    const query = new URL(req.url, 'http://localhost').searchParams
    if (req.url.startsWith('/__book.pdf')) { res.setHeader('Content-Type', 'application/pdf'); res.end(query.has('original') && process.env.ARGUS_TEST_PDF ? await readFile(process.env.ARGUS_TEST_PDF) : fixturePdf(query.get('seed') ?? '')); return }
    if (!req.url.startsWith('/__viewer')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>html,body,#app{margin:0;height:100%;width:100%;overflow:hidden}*{box-sizing:border-box}</style>
      <script type="module">
      import{createApp,h,reactive,shallowRef}from'vue';import{createPinia}from'pinia';
      import PdfViewer from'/src/components/PdfViewer.vue';import{ i18n }from'/src/i18n/index.ts';
      import{pdfDocumentCacheKey,readPdfPageCache,pdfPageCacheKey,writePdfPageCache}from'/src/utils/pdfPageCache.ts';
      import*as limitedCache from'/src/utils/pdfPageCache.ts?cache-limits-test';
      import'/src/assets/main.css';import'/src/assets/themes.css';import'pdfjs-dist/legacy/web/pdf_viewer.css';
      const data=Array.from(new Uint8Array(await(await fetch('/__book.pdf'+location.search)).arrayBuffer()));
      localStorage.setItem('argus:pdf-zoom:book','2.28');
      const t=window.__test={calls:[],paints:[],cleanups:[],canvases:[],holdAnnotations:new Set(),blocked:new Map(),delay:Number(new URLSearchParams(location.search).get('delay')||0),reader:reactive({activeSlug:'book',tabs:[{slug:'book',title:'Book'}],pendingPageJump:null,
        highlightsFor:()=>[],readingStateFor:()=>({page:1,scroll_ratio:0}),setHighlights(){},setReadingState(){},persistReadingState(){},discardTabState(){},
        setPdfDoc(doc){
          const get=doc.getPage.bind(doc);doc.getPage=async(n)=>{
            const p=await get(n);if(p.__observed)return p;p.__observed=true;
            const render=p.render.bind(p),cleanup=p.cleanup.bind(p),annotations=p.getAnnotations.bind(p);
            p.cleanup=(...args)=>{t.cleanups.push({n,at:performance.now()});return cleanup(...args)};
            p.getAnnotations=async(...args)=>{if(t.holdAnnotations.has(n))await new Promise(r=>t.blocked.set(n,r));return annotations(...args)};
            p.render=(...args)=>{const at=performance.now(),task=render(...args);t.paints.push({n,at});t.canvases.push(args[0].canvas);
              if(!t.delay)return task;
              let cancel;const cancelled=new Promise((_,reject)=>{cancel=()=>{const e=new Error('cancelled');e.name='RenderingCancelledException';reject(e)}});
              return{promise:Promise.race([task.promise.then(()=>new Promise(r=>setTimeout(r,t.delay))),cancelled]),cancel(){task.cancel();cancel()}}};
            return p;
          };
        }}),library:reactive({papers:[]}),speech:reactive({isReading:false,state:'idle',playhead:null,isReadingPaper:()=>false,isReadingText:()=>false,stop(){}}),
        async invoke(name,args){t.calls.push({name,args});
          if(name==='read_pdf_bytes')return data;
          if(name==='get_highlights')return [];
          if(name==='get_reading_state')return null;
          if(name==='get_paper_status')return {text_extracted:true};
          if(name==='get_sections')return {sections:[{title:'Existing',page:1}]};
          return null;
        }};
      const component=shallowRef();const app=createApp({render:()=>h(PdfViewer,{slug:'book',ref:component})});
      app.use(createPinia());app.use(i18n);app.mount('#app');t.component=()=>component.value;
      t.visible=()=>{const c=document.querySelector('.pdf-container'),box=c.getBoundingClientRect();return [...document.querySelectorAll('.page-wrapper')].filter(p=>{const r=p.getBoundingClientRect();return r.bottom>box.top&&r.top<box.bottom}).map(p=>({index:+p.dataset.pageIndex,painted:!!p.querySelector('.pdf-canvas')}))};
      t.scroll=idx=>{const c=document.querySelector('.pdf-container'),p=document.querySelector('[data-page-index="'+idx+'"]');c.scrollTop=p.offsetTop-3};
      t.cached=async idx=>{const p=document.querySelector('[data-page-index="'+idx+'"]'),box=p.getBoundingClientRect();return !!await readPdfPageCache(pdfPageCacheKey(component.value.snapshot().document,idx,Math.round(box.width*devicePixelRatio),Math.round(box.height*devicePixelRatio)))};
      t.cacheKeys=()=>new Promise(resolve=>{const r=indexedDB.open('argus-pdf-pages-v1',1);r.onsuccess=()=>{const db=r.result,tx=db.transaction(['pages','usage']);const keys=tx.objectStore('pages').getAllKeys(),usage=tx.objectStore('usage').get('total');tx.oncomplete=()=>{resolve({keys:keys.result,usage:usage.result});db.close()}}});
      t.pdfDocumentCacheKey=pdfDocumentCacheKey;t.writePdfPageCache=writePdfPageCache;t.pageCacheKey=pdfPageCacheKey;t.limitedCache=limitedCache;
      t.ready=true;
      </script>`))
  })
  await server.listen(); origin='http://127.0.0.1:'+server.httpServer.address().port
  profile=await mkdtemp('/private/tmp/argus-pdf-scroll-')
  await startBrowser()
})
async function startBrowser(){
  browser=await chromium.launchPersistentContext(profile,{channel:'chrome',headless:true,viewport:{width:1300,height:960},deviceScaleFactor:2});page=await browser.newPage()
  page.on('pageerror',e=>{errors.push(e.message);console.error(e.message)})
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
}
after(async()=>{await browser?.close();await server?.close();if(profile)await rm(profile,{recursive:true,force:true})})
async function open(query=''){await page.goto(origin+'/__viewer'+query);await page.waitForFunction(()=>window.__test?.component()?.snapshot().rendered.length>0,null,{timeout:15000}).catch(async e=>{console.log(await page.evaluate(()=>({html:document.body.innerText,calls:window.__test?.calls,snapshot:window.__test?.component()?.snapshot(),box:document.querySelector('.pdf-container')?.getBoundingClientRect().toJSON()})));throw e})}
async function settled(){await page.waitForFunction(()=>{const s=window.__test.component().snapshot();return !s.inFlight.length&&!s.queued.length})}
async function scroll(idx){await page.evaluate(idx=>window.__test.scroll(idx),idx);await page.waitForFunction(idx=>window.__test.component().snapshot().rendered.includes(idx),idx)}
async function bitmapDigest(idx){return page.locator(`[data-page-index="${idx}"] .pdf-canvas`).evaluate(async image=>{
  const canvas=document.createElement('canvas');canvas.width=image.naturalWidth||image.width;canvas.height=image.naturalHeight||image.height;
  const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
  const hash=Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256',pixels))).map(b=>b.toString(16).padStart(2,'0')).join('');
  const size=[canvas.width,canvas.height];canvas.width=canvas.height=0;return {hash,size};
})}

test('high zoom preloads complete next pages at exact display resolution',async()=>{
  await open('?seed=ahead');await settled()
  assert.ok((await page.evaluate(()=>window.__test.component().snapshot())).rendered.includes(2))
  const bitmap=await page.locator('[data-page-index="0"] .pdf-canvas').evaluate(canvas=>({width:canvas.width,height:canvas.height,css:canvas.getBoundingClientRect().toJSON(),dpr:devicePixelRatio}))
  assert.equal(bitmap.width,Math.round(bitmap.css.width*bitmap.dpr));assert.equal(bitmap.height,Math.round(bitmap.css.height*bitmap.dpr))
  assert.match(await page.locator('[data-page-index="0"] .textLayer').textContent(),/Page 1 ahead/)
})
test('returning to a recently viewed page reuses its bitmap without another decode',async()=>{
  await open('?seed=return');await settled()
  const before=await page.evaluate(()=>{window.__test.first=document.querySelector('[data-page-index="0"] .pdf-canvas');return window.__test.paints.filter(p=>p.n===1).length})
  await scroll(8);await settled();await scroll(0);await settled()
  assert.equal(await page.evaluate(()=>window.__test.first===document.querySelector('[data-page-index="0"] .pdf-canvas')),true)
  assert.equal(await page.evaluate(()=>window.__test.paints.filter(p=>p.n===1).length),before)
  assert.ok((await page.evaluate(()=>window.__test.component().snapshot())).pixels<=100_000_000)
})
test('a newly visible page overtakes two unfinished offscreen prefetches',async()=>{
  await open('?seed=priority');await settled()
  await page.evaluate(()=>{window.__test.holdAnnotations.add(10).add(11);window.__test.scroll(8)})
  await page.waitForFunction(()=>window.__test.blocked.size===2)
  await page.evaluate(()=>window.__test.scroll(7))
  await page.waitForFunction(()=>window.__test.component().snapshot().rendered.includes(7),null,{timeout:2000})
  assert.equal(await page.evaluate(()=>window.__test.blocked.size),2)
  await page.evaluate(()=>{window.__test.holdAnnotations.clear();for(const resolve of window.__test.blocked.values())resolve()})
  await settled()
})
test('evicted and cancelled canvases release their backing stores immediately',async()=>{
  await open('?seed=release&delay=180');await settled()
  for(const idx of [8,16,24,4])await scroll(idx)
  await settled()
  const resources=await page.evaluate(()=>window.__test.canvases.filter(c=>!c.isConnected).map(c=>[c.width,c.height]))
  assert.ok(resources.length>0);assert.ok(resources.every(([w,h])=>!w&&!h),JSON.stringify(resources))
  assert.ok((await page.evaluate(()=>window.__test.component().snapshot())).pixels<=100_000_000)
})
test('persisted pages survive reopening and invalidate on zoom or changed file content',async()=>{
  await open('?seed=persist&delay=150');await settled()
  await page.waitForFunction(async()=>[await window.__test.cached(0),await window.__test.cached(1),await window.__test.cached(2)].every(Boolean))
  const old=await page.evaluate(()=>window.__test.component().snapshot().document)
  const pixels=await bitmapDigest(0)
  // A fresh process using the same isolated profile proves this is disk
  // persistence, rather than an in-memory cache surviving page navigation.
  await browser.close();await startBrowser()
  await open('?seed=persist&delay=150');await settled()
  assert.equal(await page.evaluate(()=>window.__test.paints.length),0)
  assert.equal(await page.locator('[data-page-index="0"] .pdf-canvas').evaluate(img=>img.tagName),'IMG')
  assert.deepEqual(await bitmapDigest(0),pixels)
  assert.match(await page.locator('[data-page-index="0"] .textLayer').textContent(),/Page 1 persist/)
  await page.evaluate(()=>window.__test.component().scale=2.5);await page.waitForFunction(()=>window.__test.paints.length>0);await settled()
  await open('?seed=replaced&delay=150');await settled()
  assert.notEqual(await page.evaluate(()=>window.__test.component().snapshot().document),old)
  assert.ok(await page.evaluate(()=>window.__test.paints.length)>0)
})
test('a damaged cached PNG is discarded and rendered again',async()=>{
  await open('?seed=corrupt&delay=150');await settled();await page.waitForFunction(()=>window.__test.cached(0))
  await page.evaluate(async()=>{
    const t=window.__test,p=document.querySelector('[data-page-index="0"]'),box=p.getBoundingClientRect();
    const key=t.pageCacheKey(t.component().snapshot().document,0,Math.round(box.width*devicePixelRatio),Math.round(box.height*devicePixelRatio));
    await t.writePdfPageCache(key,new Blob(['broken png'],{type:'image/png'}));
  })
  await open('?seed=corrupt&delay=150');await settled()
  assert.ok(await page.evaluate(()=>window.__test.paints.some(p=>p.n===1)))
  assert.match(await page.locator('[data-page-index="0"] .textLayer').textContent(),/Page 1 corrupt/)
})
test('persistent cache evicts least-recently-used entries within byte and entry limits',async()=>{
  const result=await page.evaluate(async()=>{
    const c=window.__test.limitedCache,blob=new Blob([new Uint8Array(400000)],{type:'image/png'});
    await c.writePdfPageCache('a',blob);await c.writePdfPageCache('b',blob);await c.writePdfPageCache('c',blob);
    const first=!!await c.readPdfPageCache('a');await c.readPdfPageCache('b');
    await c.writePdfPageCache('d',blob);const second=!!await c.readPdfPageCache('c'),kept=!!await c.readPdfPageCache('b');
    await c.writePdfPageCache('b',new Blob([new Uint8Array(200000)],{type:'image/png'}));await c.deletePdfPageCache('d');
    for(const key of ['e','f','g','h'])await c.writePdfPageCache(key,new Blob(['png'],{type:'image/png'}));
    const summary=await new Promise(resolve=>{const r=indexedDB.open('argus-pdf-pages-limits-test',1);r.onsuccess=()=>{const db=r.result,tx=db.transaction(['pages','usage']);const keys=tx.objectStore('pages').getAllKeys(),usage=tx.objectStore('usage').get('total');tx.oncomplete=()=>{resolve({keys:keys.result,usage:usage.result});db.close()}}});
    return {first,second,kept,summary};
  })
  assert.equal(result.first,false);assert.equal(result.second,false);assert.equal(result.kept,true)
  assert.equal(result.summary.usage.count,3);assert.equal(result.summary.usage.bytes,9)
  assert.equal(result.summary.keys.length,3)
})
test('a blocked cache times out without blocking rendering or retaining writes forever',async()=>{
  const result=await page.evaluate(async()=>{
    const c=await import('/src/utils/pdfPageCache.ts?blocked-test'),open=indexedDB.open;
    indexedDB.open=()=>({});
    try{
      const at=performance.now(),blob=await c.readPdfPageCache('blocked'),readMs=performance.now()-at;
      await c.writePdfPageCache('blocked',new Blob(['png'],{type:'image/png'}));
      return {miss:blob===null,readMs,writeMs:performance.now()-at};
    }finally{indexedDB.open=open}
  })
  assert.equal(result.miss,true);assert.ok(result.readMs<1000,JSON.stringify(result));assert.ok(result.writeMs<2500,JSON.stringify(result))
  assert.deepEqual(errors,[])
})

test('the original scanned book scrolls and reopens from persisted bitmaps',{skip:!process.env.ARGUS_TEST_PDF},async()=>{
  await open('?original=1')
  const result=await page.evaluate(async()=>{
    const t=window.__test,c=document.querySelector('.pdf-container');
    await new Promise(r=>setTimeout(r,1000));
    let last=performance.now(),blankMs=0,frames=0;
    await new Promise(resolve=>{
      let travel=0;
      function frame(now){const dt=now-last;last=now;frames++;
        if(t.visible().some(p=>!p.painted))blankMs+=dt;
        travel+=dt*5;c.scrollTop=travel;
        if(travel<35000)requestAnimationFrame(frame);else resolve();
      }requestAnimationFrame(frame)
    });
    const snap=t.component().snapshot(),at=performance.now();
    while(t.visible().some(p=>!p.painted)&&performance.now()-at<10000)await new Promise(r=>setTimeout(r,20));
    return {blankMs,frames,settle:performance.now()-at,snapshot:snap,visible:t.visible()};
  })
  console.log(JSON.stringify(result))
  await scroll(19);await settled();await page.waitForFunction(()=>window.__test.cached(19))
  const pixels=await bitmapDigest(19)
  if(process.env.ARGUS_TEST_OUTPUT_DIR){
    // Capture pixels directly: an element screenshot clips a tall page at the
    // reader's overflow viewport, leaving an artificial white lower half.
    const png=await page.locator('[data-page-index="19"] .pdf-canvas').evaluate(image=>{
      const c=document.createElement('canvas');c.width=image.naturalWidth||image.width;c.height=image.naturalHeight||image.height;
      c.getContext('2d').drawImage(image,0,0);const data=c.toDataURL('image/png').split(',')[1];c.width=c.height=0;return data;
    });await writeFile(process.env.ARGUS_TEST_OUTPUT_DIR+'/scroll-page-20.png',Buffer.from(png,'base64'));
  }
  await open('?original=1');await scroll(19);await settled()
  assert.equal(await page.evaluate(()=>window.__test.paints.filter(p=>p.n===20).length),0)
  assert.equal(await page.locator('[data-page-index="19"] .pdf-canvas').evaluate(img=>img.tagName),'IMG')
  assert.deepEqual(await bitmapDigest(19),pixels)
  assert.deepEqual(errors,[])
})
