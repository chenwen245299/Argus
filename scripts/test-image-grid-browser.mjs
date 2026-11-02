// Real chat grid, image loading, enlarged-image actions and Markdown renderer.
// Local fixtures and clipboard mocks only; no library or system clipboard access.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const mocks = {
  '@tauri-apps/api/core': `export const isTauri=()=>false;export const invoke=async(command,args)=>{window.__test.reads.push({command,args});return window.__test.pngs[args.file]};`,
  '@tauri-apps/api/image': `export class Image{}`,
  '@tauri-apps/plugin-clipboard-manager': `export const writeImage=async()=>{};export const writeText=async()=>{};`,
}
let server, browser, page, origin
const errors = []
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'image-grid-ipc', enforce: 'pre',
      resolveId: source => source in mocks ? '\0mock:' + source : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null,
    }, vue()], cacheDir: '/private/tmp/argus-image-grid-vite',
    optimizeDeps: { entries: ['src/components/ChatAnswerBody.vue', 'src/components/ImagePreviewActions.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req,res,next) => {
    if (!req.url.startsWith('/__image-grid-test')) return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><div id="app"></div>
      <style>body{margin:0;padding:20px;box-sizing:border-box}#app{height:100%;overflow:auto}.answer{max-width:720px}
      .lightbox{position:fixed;inset:0;background:#000a;display:flex;align-items:center;justify-content:center;z-index:1000}
      .lightbox>img{max-width:90vw;max-height:85vh}</style><script type="module">
      import{createApp,h,ref,nextTick}from'vue';import{createI18n}from'vue-i18n';import{addCollection}from'@iconify/vue';
      import fluent from'@iconify-json/fluent/icons.json';import{zh,en}from'/src/i18n/locales.ts';
      import Body from'/src/components/ChatAnswerBody.vue';import Actions from'/src/components/ImagePreviewActions.vue';
      import{renderMarkdown}from'/src/utils/renderMarkdown.ts';import{fragmentToMarkdown}from'/src/utils/copyAsMarkdown.ts';
      import'/src/assets/main.css';import'/src/assets/themes.css';addCollection(fluent);
      const dimensions=[[640,360],[400,600],[640,360],[600,400]],colors=['#e1edf7','#e1f1e8','#fbebdd','#eee5f6'],pngs={};
      const images=dimensions.map(([width,height],i)=>{const canvas=document.createElement('canvas');canvas.width=width;canvas.height=height;
        const c=canvas.getContext('2d');c.fillStyle=colors[i];c.fillRect(0,0,width,height);c.fillStyle='#36516c';c.font='bold 36px sans-serif';c.fillText('Figure '+(i+1),35,60);
        c.strokeStyle='#6285a5';c.lineWidth=5;c.beginPath();c.moveTo(45,height-50);c.lineTo(width-50,100);c.stroke();
        const file='image-'+i+'.png';pngs[file]=canvas.toDataURL();return{file,slug:'paper',page:i+1,width,height,caption:'图 '+(i+1)+'：关键实验与方法',dataUrl:pngs[file]}});
      const saved=JSON.parse(localStorage.getItem('image-grid-fixture')||'null');
      const content=ref(saved?.content||''),steps=ref(saved?.steps||[{tool:'send_paper_image',ok:true,images}]),preview=ref(null),streaming=ref(false);
      const test=window.__test={pngs,images,reads:[],copied:[],renderMarkdown,fragmentToMarkdown,
        layout:async(rows,columns)=>{const ref=i=>'![](argus-image://image-'+i+'.png)';const row=r=>'| '+Array.from({length:columns},(_,c)=>ref(r*columns+c)).join(' | ')+' |';
          content.value='## 对比分析\\n\\n下面将图片按 '+rows+'×'+columns+' 排列。\\n\\n'+[row(0),'| '+Array(columns).fill('---').join(' | ')+' |',...Array.from({length:rows-1},(_,r)=>row(r+1))].join('\\n')+'\\n\\n对应解释与公式 $x^2$。';await nextTick()},
        save:()=>localStorage.setItem('image-grid-fixture',JSON.stringify({content:content.value,steps:steps.value.map(step=>({...step,images:step.images.map(({dataUrl,...image})=>image)}))})),
        stream:value=>streaming.value=value,
      };
      window.ClipboardItem=class{constructor(data){this.data=data}};Object.defineProperty(navigator,'clipboard',{value:{write:async items=>{
        const blob=await items[0].data['image/png'],image=await createImageBitmap(blob);test.copied.push({width:image.width,height:image.height,type:blob.type});image.close()}}});
      createApp({setup(){return()=>h('div',{},[h('article',{class:'markdown-body answer'},[h(Body,{content:content.value,steps:steps.value,conversationId:'fixture-chat',streaming:streaming.value,onOpen:src=>preview.value=src})]),
        preview.value?h('div',{class:'lightbox',onClick:event=>{if(event.target===event.currentTarget)preview.value=null}},[
          h('img',{src:preview.value}),h(Actions,{src:preview.value,onClose:()=>preview.value=null})]):null])}})
        .use(createI18n({legacy:false,locale:'zh',messages:{zh,en}})).mount('#app');test.ready=true;
    </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true})
  page=await browser.newPage({viewport:{width:820,height:1100}})
  page.setDefaultTimeout(12000)
  page.on('pageerror',error=>errors.push(error.message))
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
})
after(async()=>{await browser?.close();await server?.close()})
async function open(rows,columns){
  await page.goto(origin+'/__image-grid-test');await page.evaluate(()=>localStorage.clear());await page.reload()
  await page.waitForFunction(()=>window.__test?.ready)
  await page.evaluate(([rows,columns])=>window.__test.layout(rows,columns),[rows,columns])
  await ready(rows*columns)
}
async function ready(count){await page.waitForFunction(count=>{
  const images=[...document.querySelectorAll('.chat-answer-image-grid img')];return images.length===count&&images.every(image=>image.complete&&image.naturalWidth>0)
},count)}
async function positions(){return page.locator('.chat-answer-image-grid .chat-image-message').evaluateAll(images=>images.map(image=>{
  const b=image.getBoundingClientRect();return{x:b.x,y:b.y,width:b.width,height:b.height}
}))}

test('1x2 displays two equal columns beside the text and each original can still be enlarged and copied',async()=>{
  await open(1,2)
  const boxes=await positions();assert.equal(boxes[0].y,boxes[1].y);assert.ok(boxes[1].x>boxes[0].x+boxes[0].width)
  assert.ok(Math.abs(boxes[0].width-boxes[1].width)<1)
  assert.equal(await page.locator('.chat-answer-image-grid').count(),1)
  const ratios=await page.locator('.chat-answer-image-grid .chat-image-open').evaluateAll(buttons=>buttons.map(button=>button.clientWidth/button.clientHeight))
  assert.ok(ratios.every(ratio=>Math.abs(ratio-4/3)<.02),JSON.stringify(ratios))
  assert.ok(await page.locator('.answer .katex').count())
  await page.locator('.chat-answer-image-grid .chat-image-open').nth(1).click()
  await page.getByRole('button',{name:'复制图片',exact:true}).click()
  await page.getByRole('status').waitFor()
  assert.deepEqual(await page.evaluate(()=>window.__test.copied),[{width:400,height:600,type:'image/png'}])
  await page.getByRole('button',{name:'关闭图片预览'}).click()
  if(process.env.ARGUS_TEST_OUTPUT_DIR){await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true});await page.locator('.answer').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/chat-grid-1x2.png'})}
})

test('2x2 preserves order through save/reload and reloads each conversation image only once',async()=>{
  await open(2,2)
  let boxes=await positions()
  assert.equal(boxes[0].y,boxes[1].y);assert.equal(boxes[2].y,boxes[3].y)
  assert.ok(boxes[2].y>boxes[0].y+boxes[0].height)
  const heights=await page.locator('.chat-answer-image-grid .chat-image-open').evaluateAll(buttons=>buttons.map(button=>button.clientHeight))
  assert.ok(Math.max(...heights)-Math.min(...heights)<2,JSON.stringify(heights))
  const before=await page.locator('.chat-answer-image-grid figcaption').allTextContents()
  await page.evaluate(()=>window.__test.save());await page.reload();await ready(4)
  assert.deepEqual(await page.locator('.chat-answer-image-grid figcaption').allTextContents(),before)
  assert.deepEqual(await page.evaluate(()=>window.__test.reads.map(read=>read.args.file)),['image-0.png','image-1.png','image-2.png','image-3.png'])
  assert.equal(await page.locator('.chat-answer-images').count(),0)
  if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.locator('.answer').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/chat-grid-2x2.png'})
  await page.setViewportSize({width:380,height:1000});await page.evaluate(()=>document.documentElement.dataset.theme='dark')
  boxes=await positions();assert.equal(boxes[0].y,boxes[1].y);assert.equal(boxes[2].y,boxes[3].y)
  assert.ok(boxes.every(box=>box.x>=0&&box.x+box.width<=380))
  assert.equal(await page.locator('.answer').evaluate(el=>el.scrollWidth>el.clientWidth),false)
  if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.locator('.answer').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/chat-grid-narrow.png'})
  await page.setViewportSize({width:820,height:1100})
})

test('image-only Markdown tables keep their grid when copied, while data tables and code keep normal rendering',async()=>{
  await open(1,2)
  const result=await page.evaluate(()=>{
    const t=window.__test,ref=i=>'![图 '+i+']('+t.pngs['image-'+i+'.png']+')';
    const source='| '+ref(0)+' | '+ref(1)+' |\n| --- | --- |\n| '+ref(2)+' | '+ref(3)+' |';
    const div=document.createElement('div');div.innerHTML=t.renderMarkdown(source)
    const copied=t.fragmentToMarkdown(div),reload=document.createElement('div');reload.innerHTML=t.renderMarkdown(copied)
    const data=t.renderMarkdown('| 指标 | 结果 |\n| --- | --- |\n| Accuracy | 90% |')
    const code=t.renderMarkdown('```markdown\n'+source+'\n```')
    return{grids:reload.querySelectorAll('table.md-image-grid').length,rows:reload.querySelectorAll('tr').length,images:reload.querySelectorAll('img').length,
      dataHeader:data.includes('<thead>'),dataGrid:data.includes('md-image-grid'),codeGrid:code.includes('<table')}
  })
  assert.deepEqual(result,{grids:1,rows:2,images:4,dataHeader:true,dataGrid:false,codeGrid:false})
})

test('no browser errors',()=>assert.deepEqual(errors,[]))
