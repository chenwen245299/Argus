// Real approval card, Markdown renderer, note asset resolver and Vditor editor.
// Local mock library only; no API, real library or system clipboard access.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'note-image-ipc', enforce: 'pre',
      resolveId: source => source === '@tauri-apps/api/core' ? '\0note-image-ipc' : null,
      load: id => id === '\0note-image-ipc' ? 'export const invoke=(...args)=>window.__test.invoke(...args);' : null,
    }, vue()], optimizeDeps: { entries: ['src/components/WriteConfirmCard.vue', 'src/components/VditorEditor.vue'] },
    cacheDir: '/private/tmp/argus-note-images-vite', server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__note-images')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><div id="app"></div>
      <style>body{margin:0;padding:20px;box-sizing:border-box}#app{max-width:620px}.editor{height:600px}</style><script type="module">
      import{createApp,h,ref,nextTick}from'vue';import{createI18n}from'vue-i18n';
      import Card from'/src/components/WriteConfirmCard.vue';import Editor from'/src/components/VditorEditor.vue';
      import{toDisplayMarkdown,toStoredMarkdown}from'/src/utils/noteAssets.ts';import{zh,en}from'/src/i18n/locales.ts';
      import'/src/assets/main.css';import'/src/assets/themes.css';
      const canvas=document.createElement('canvas');canvas.width=600;canvas.height=240;
      const ctx=canvas.getContext('2d');ctx.fillStyle='#edf3f9';ctx.fillRect(0,0,600,240);
      ctx.fillStyle='#376286';ctx.fillRect(40,70,150,85);ctx.fillRect(360,70,190,85);
      ctx.fillStyle='#222';ctx.font='22px sans-serif';ctx.fillText('Figure 1: input → representation',35,35);
      ctx.fillStyle='white';ctx.fillText('Input',85,120);ctx.fillText('Features',405,120);
      const png=canvas.toDataURL('image/png'),path='assets/ai-1234-abcd.png';
      const content='# 方法笔记\\n\\n先看论文中的关键设计。\\n\\n![图 1：方法结构]('+path+')\\n\\n这张图说明输入如何变成表示。公式 $x^2$。';
      const preview=ref({tool:'create_paper_note',slug:'paper',paperTitle:'示例论文',noteTitle:'方法图解',content,duplicateTitle:false,noteImages:[{path,dataUrl:png}]});
      const mode=ref('preview'),display=ref(''),draft=ref(''),visible=ref(true);
      const urls=[],revoked=[];const create=URL.createObjectURL.bind(URL),revoke=URL.revokeObjectURL.bind(URL);
      URL.createObjectURL=blob=>{const url=create(blob);urls.push(url);return url};URL.revokeObjectURL=url=>{revoked.push(url);revoke(url)};
      const disk=new Map(),calls=[],decisions=[];
      const test=window.__test={preview,mode,display,draft,urls,revoked,calls,decisions,content,png,path,
        invoke(command,args){calls.push({command,args});if(command==='read_note_asset'){
          if(test.delayRead)return new Promise(resolve=>test.resolveRead=()=>resolve(png.split(',')[1]));
          const value=disk.get(args.name);return value?Promise.resolve(value):Promise.reject('Missing image');}
          return Promise.resolve(null);},
        async reopen(){display.value=await toDisplayMarkdown('paper',draft.value);mode.value='editor'},
        async freshPreview(value){mode.value='preview';preview.value=value;await nextTick()},
        hide(){visible.value=false},
        setDisk(name){disk.set(name,png.split(',')[1])},
        stored(){return toStoredMarkdown(draft.value)},
      };
      createApp({setup(){return()=>visible.value?(mode.value==='preview'?h(Card,{preview:preview.value,
        onApprove:()=>{decisions.push('approved');for(const image of preview.value.noteImages||[])disk.set(image.path.slice(7),image.dataUrl.split(',')[1]);draft.value=preview.value.content;test.reopen()},
        onReject:()=>decisions.push('rejected')}):h('div',{class:'editor'},[h(Editor,{initialContent:display.value,onChange:value=>draft.value=toStoredMarkdown(value)})])):null}})
        .use(createI18n({legacy:false,locale:'zh',messages:{zh,en}})).mount('#app');test.ready=true;
    </script>`))
  })
  await server.listen(); origin = 'http://127.0.0.1:' + server.httpServer.address().port
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 780, height: 800 } })
  page.setDefaultTimeout(15000)
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await server?.close() })
async function open() {
  await page.goto(origin + '/__note-images')
  await page.waitForFunction(() => window.__test?.ready)
}
async function loadedImages(selector, count) {
  await page.waitForFunction(({ selector, count }) => {
    const images = [...document.querySelectorAll(selector)]
    return images.length === count && images.every(image => image.complete && image.naturalWidth === 600)
  }, { selector, count })
}

test('approval renders captured figures inline; saving and reopening uses permanent note assets in the real editor', async () => {
  await open(); await loadedImages('.wc-body img', 1)
  assert.match(await page.locator('.wc-body img').getAttribute('src'), /^blob:/)
  assert.equal(await page.locator('.wc-body img').getAttribute('alt'), '图 1：方法结构')
  assert.deepEqual(await page.evaluate(() => window.__test.calls), [], 'pending pixels need no disk write/read')
  assert.deepEqual(await page.evaluate(() => window.__test.decisions), [])
  assert.ok(await page.locator('.wc-body .katex').count())
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.locator('#app').screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/note-images-preview.png' })
  }
  await page.locator('.wc-btn.primary').click()
  await loadedImages('.vditor-ir img', 1)
  assert.match(await page.locator('.vditor-ir img').getAttribute('src'), /^blob:/)
  const state = await page.evaluate(() => ({ stored: window.__test.stored(), calls: window.__test.calls, decisions: window.__test.decisions }))
  assert.match(state.stored, /!\[图 1：方法结构\]\(assets\/ai-1234-abcd.png\)/)
  assert.doesNotMatch(state.stored, /blob:|base64|argus-note-image:|argus-image:/)
  assert.deepEqual(state.decisions, ['approved'])
  assert.deepEqual(state.calls, [{ command: 'read_note_asset', args: { slug: 'paper', name: 'ai-1234-abcd.png' } }])
  if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.locator('#app').screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/note-images-editor.png' })
})

test('an existing note preview shows saved and new images; rejection creates no assets', async () => {
  await open()
  await page.evaluate(async () => {
    const t = window.__test
    t.setDisk('old.png')
    await t.freshPreview({ ...t.preview.value, tool: 'update_paper_note',
      content: '保留原图。\n\n![旧图](assets/old.png)\n\n新图解释。\n\n![新图](' + t.path + ')',
      noteBefore: { id: 'note', title: '原笔记', content: '![旧图](assets/old.png)' } })
  })
  await loadedImages('.wc-note-before img', 1); await loadedImages('.wc-note-after img', 2)
  await page.locator('.wc-btn').first().click()
  assert.deepEqual(await page.evaluate(() => window.__test.decisions), ['rejected'])
  assert.ok((await page.evaluate(() => window.__test.calls)).every(call => call.command === 'read_note_asset'))
  await page.setViewportSize({ width: 380, height: 740 })
  if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.locator('#app').screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/note-images-narrow.png' })
  const overflow = await page.evaluate(() => ({ width: innerWidth, scroll: document.documentElement.scrollWidth,
    elements: [...document.querySelectorAll('body *')].filter(el => el.getBoundingClientRect().right > innerWidth).map(el => [el.tagName,el.className,el.getBoundingClientRect().right]) }))
  assert.equal(overflow.scroll > overflow.width, false, JSON.stringify(overflow))
  await page.setViewportSize({ width: 780, height: 800 })
})

test('changing a preview ignores old asset reads and releases pending image URLs on unmount', async () => {
  await open(); await loadedImages('.wc-body img', 1)
  await page.evaluate(async () => {
    const t = window.__test
    t.delayRead = true
    await t.freshPreview({ ...t.preview.value, content: '旧预览\n\n![旧图](assets/slow.png)', noteImages: [] })
  })
  await page.waitForFunction(() => window.__test.resolveRead)
  await page.evaluate(async () => {
    const t = window.__test
    await t.freshPreview({ ...t.preview.value, content: '新的纯文字预览', noteImages: [] })
    t.resolveRead()
  })
  await page.getByText('新的纯文字预览', { exact: true }).waitFor()
  assert.equal(await page.locator('.wc-body img').count(), 0)
  await page.evaluate(() => window.__test.hide())
  const state = await page.evaluate(() => ({ created: window.__test.urls, revoked: window.__test.revoked }))
  // The saved-asset resolver owns its cached URLs; pending approval blobs are
  // the first URLs and must be released as soon as their preview is replaced.
  assert.ok(state.revoked.includes(state.created[0]))
})


for (const rows of [1, 2]) {
  test(`${rows}x2 note grid is previewed, saved and reopened as an editable Markdown table`, async () => {
    await open()
    await page.setViewportSize({ width: 380, height: 800 })
    await page.evaluate(async rows => {
      const t=window.__test
      const images=Array.from({length:rows*2},(_,i)=>({path:'assets/ai-'+(i+1)+'.png',dataUrl:t.png}))
      const refs=images.map((image,i)=>'![图 '+(i+1)+']('+image.path+')')
      const row=(a,b)=>'| '+a+' | '+b+' |'
      const content='## 对比图\n\n'+[row(refs[0],refs[1]),'| --- | --- |',...(rows===2?[row(refs[2],refs[3])]:[])].join('\n')+'\n\n逐图解释。'
      t.gridContent=content
      await t.freshPreview({...t.preview.value,content,noteImages:images})
    }, rows)
    await loadedImages('.wc-body table.md-image-grid img', rows*2)
    assert.equal(await page.locator('.wc-body table.md-image-grid tr').count(), rows)
    assert.equal(await page.locator('.wc-body table.md-image-grid tr').first().locator('td').count(), 2)
    const previewWidths=await page.locator('.wc-body table.md-image-grid').evaluate(table=>({table:table.getBoundingClientRect().width,parent:table.parentElement.getBoundingClientRect().width}))
    assert.ok(previewWidths.table<=previewWidths.parent+1)
    await page.locator('.wc-btn.primary').click()
    await loadedImages('.vditor-ir table img', rows*2)
    assert.equal(await page.locator('.vditor-ir table tr').count(), rows)
    const widths=await page.locator('.vditor-ir table').evaluate(table=>({table:table.getBoundingClientRect().width,parent:table.parentElement.getBoundingClientRect().width,
      cells:[...table.rows[0].cells].map(cell=>cell.getBoundingClientRect().width)}))
    assert.ok(widths.table<=widths.parent+1,JSON.stringify(widths))
    assert.ok(Math.abs(widths.cells[0]-widths.cells[1])<2,JSON.stringify(widths))
    assert.equal(await page.evaluate(()=>window.__test.stored()),await page.evaluate(()=>window.__test.gridContent))
    if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.locator('#app').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/note-grid-'+rows+'x2.png'})
    // Trigger a genuine editor input and check Lute's Markdown round trip.
    await page.locator('.vditor-ir .vditor-reset').click({position:{x:30,y:20}})
    await page.keyboard.press('ControlOrMeta+End')
    await page.keyboard.type(' edited')
    await page.waitForFunction(()=>window.__test.stored().includes('edited'))
    const saved=await page.evaluate(()=>window.__test.stored())
    assert.doesNotMatch(saved,/blob:|base64/)
    assert.equal((saved.match(/assets\/ai-\d\.png/g)||[]).length,rows*2)
    await page.setViewportSize({ width: 780, height: 800 })
  })
}

test('no browser errors', () => assert.deepEqual(errors, []))
