// Real Vditor/Markdown rendering and undo; only the macOS handoff is mocked.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
before(async () => {
  const mocks = {
    '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args)`,
    '@tauri-apps/plugin-dialog': `export const message=async text=>window.__test.messages.push(text)`,
  }
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    cacheDir: '/private/tmp/argus-writing-tools-vite',
    plugins: [{ name: 'writing-tools-native-mock', enforce: 'pre',
      resolveId: id => id in mocks ? '\0writing-tools:' + id : null,
      load: id => id.startsWith('\0writing-tools:') ? mocks[id.slice(15)] : null,
    }, vue()], optimizeDeps: { entries: ['src/components/VditorEditor.vue', 'src/components/MarkdownBody.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__writing-tools')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><div id="app"></div>
      <style>.editor{position:relative;height:400px}.answer-body{user-select:text}body{margin:20px}</style>
      <script type="module">
      import{createApp,h,ref}from'vue';import Editor from'/src/components/VditorEditor.vue';
      import Markdown from'/src/components/MarkdownBody.vue';import{i18n}from'/src/i18n/index.ts';
      import{captureWritingToolsSelection,runWritingTools,installTextContextMenu}from'/src/utils/writingTools.ts';
      import'/src/assets/main.css';import'/src/assets/themes.css';import'katex/dist/katex.min.css';
      const editor=ref(null),scope=ref('/library-a'),shown=ref(true),answer=ref('模型的普通回答，包含 **加粗文字** 和公式 $x_i^2$。');
      const source='# 研究笔记\\n\\n第一段原始文字，需要润色。\\n\\n保留 **重点** 和 $x_i^2$。';
      const t=window.__test={messages:[],calls:[],changes:[],source,editor,scope,shown,answer,
        invoke(name,args){t.calls.push({name,args});return new Promise(resolve=>t.resolve=resolve)},
        select(selector,text){const root=document.querySelector(selector),r=document.createRange();root.focus();
          if(text){const w=document.createTreeWalker(root,NodeFilter.SHOW_TEXT);let n;while(n=w.nextNode()){const at=n.textContent.indexOf(text);if(at>=0){r.setStart(n,at);r.setEnd(n,at+text.length);break}}}
          else r.selectNodeContents(root);const s=getSelection();s.removeAllRanges();s.addRange(r);return captureWritingToolsSelection(root)},
        capture:target=>captureWritingToolsSelection(target),run:runWritingTools,
      };
      window.__TAURI_INTERNALS__={};Object.defineProperty(navigator,'platform',{value:'MacIntel'});installTextContextMenu();
      createApp({setup(){return()=>[shown.value?h('div',{class:'editor'},[h(Editor,{ref:editor,initialContent:source,libraryRoot:scope.value,onChange:md=>t.changes.push(md)})]):null,
        h('div',{class:'answer-body markdown-body'},[h(Markdown,{content:answer.value})]),h('textarea',{id:'ordinary'}),h('button',{id:'other'},'Other')]}}).use(i18n).mount('#app');t.ready=true;
      </script>`))
  })
  await server.listen(); origin = 'http://127.0.0.1:' + server.httpServer.address().port
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 1000, height: 800 } })
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
})
after(async () => { await browser?.close(); await server?.close() })
async function open() {
  await page.goto(origin + '/__writing-tools')
  await page.locator('.vditor-ir .vditor-reset').waitFor()
  await page.waitForFunction(() => window.__test.editor.value?.getContent()?.includes('第一段原始文字'))
  await page.waitForTimeout(400) // allow the real editor's initial undo snapshot
}
const root = '.vditor-ir .vditor-reset'

test('native note handoff preserves Markdown/TeX; replacing a selected paragraph emits changes and remains undoable', async () => {
  await open()
  const captured = await page.evaluate(async root => {
    const t=window.__test,selection=t.select(root,'第一段原始文字，需要润色。');
    const result=await t.run(selection,async(text,canReplace)=>{t.native={text,canReplace};return{action:'replace',text:'第一段修改后的文字，更加清楚。'}});
    return{result,native:t.native,value:t.editor.value.getContent(),changes:t.changes};
  }, root)
  assert.equal(captured.result, true)
  assert.deepEqual(captured.native, { text: '第一段原始文字，需要润色。', canReplace: true })
  assert.match(captured.value, /第一段修改后的文字，更加清楚。/)
  assert.match(captured.value, /\*\*重点\*\*/)
  assert.match(captured.value, /\$x_i\^2\$/)
  assert.equal(captured.changes.at(-1), captured.value)
  await page.locator(root).press('Meta+z')
  await page.waitForFunction(()=>window.__test.editor.value.getContent().includes('第一段原始文字，需要润色。'))
  const full = await page.evaluate(root => window.__test.select(root).text, root)
  assert.match(full, /# 研究笔记/); assert.match(full, /\*\*重点\*\*/); assert.match(full, /\$x_i\^2\$/)
})

test('AI answer selections keep formula source and cannot overwrite saved/rendered model answers', async () => {
  await open()
  const result = await page.evaluate(async()=>{
    const t=window.__test,s=t.select('.answer-body');const before=document.querySelector('.answer-body').innerHTML;
    const ok=await t.run(s,async(text,canReplace)=>{t.native={text,canReplace};return{action:'replace',text:'不要改写模型回答'}});
    return{ok,native:t.native,before,after:document.querySelector('.answer-body').innerHTML};
  })
  assert.equal(result.ok, false); assert.equal(result.native.canReplace, false)
  assert.match(result.native.text, /\*\*加粗文字\*\*/); assert.match(result.native.text, /\$x_i\^2\$/)
  assert.equal(result.after, result.before)
})

test('late results refuse changed notes, switched libraries, removed editors and moved selections', async () => {
  for (const change of ['remote', 'library', 'unmount', 'range']) {
    await open()
    const result = await page.evaluate(async({root,change})=>{
      const t=window.__test,s=t.select(root,'第一段原始文字，需要润色。');
      const pending=t.run(s,()=>new Promise(resolve=>t.finish=resolve));
      if(change==='remote')t.editor.value.setContent('在别处更新的笔记');
      if(change==='library'){t.scope.value='/library-b';await new Promise(r=>setTimeout(r,0))}
      if(change==='unmount'){t.shown.value=false;await new Promise(r=>setTimeout(r,0))}
      if(change==='range')document.querySelector(root).textContent='新的选区';
      t.finish({action:'replace',text:'过期的处理结果'});
      return{ok:await pending,body:document.body.textContent};
    },{root,change})
    assert.equal(result.ok,false,change);assert.doesNotMatch(result.body,/过期的处理结果/)
  }
})

test('copy/cancel never edit; right-click uses the captured selection, and custom/input menus retain precedence', async () => {
  await open()
  const result = await page.evaluate(async root=>{
    const t=window.__test,s=t.select(root,'第一段原始文字，需要润色。'),before=t.editor.value.getContent();
    await t.run(s,async()=>({action:'cancel'}));await t.run(s,async()=>({action:'copy'}));
    const editor=document.querySelector(root),event=new MouseEvent('contextmenu',{bubbles:true,cancelable:true});editor.dispatchEvent(event);
    await new Promise(r=>setTimeout(r,0));const request=t.calls.at(-1);t.resolve({action:'cancel'});
    editor.addEventListener('contextmenu',e=>e.preventDefault(),{once:true});editor.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true}));
    const ordinary=new MouseEvent('contextmenu',{bubbles:true,cancelable:true});document.querySelector('#ordinary').dispatchEvent(ordinary);
    const other=new MouseEvent('contextmenu',{bubbles:true,cancelable:true});document.querySelector('#other').dispatchEvent(other);
    return{before,after:t.editor.value.getContent(),request,calls:t.calls.length,prevented:event.defaultPrevented,input:ordinary.defaultPrevented,other:other.defaultPrevented};
  },root)
  assert.equal(result.after,result.before);assert.equal(result.prevented,true);assert.equal(result.calls,1)
  assert.deepEqual(result.request,{name:'system_writing_tools',args:{text:'第一段原始文字，需要润色。',canReplace:true,locale:'zh'}})
  assert.equal(result.input,false);assert.equal(result.other,true)
})

test('real macOS WKWebView passes note/answer selections through the production native panel', {skip:process.platform!=='darwin'}, async()=>{
  const directory=await mkdtemp(join(tmpdir(),'argus-writing-tools-webview-'))
  try{
    const binary=join(directory,'webview-test')
    const build=spawnSync('xcrun',['clang','-fobjc-arc','-Wall','-Werror','-mmacosx-version-min=11.0','-framework','AppKit','-framework','WebKit',new URL('./test-writing-tools-webview.m',import.meta.url).pathname,'-o',binary],{encoding:'utf8'})
    assert.equal(build.status,0,build.stderr)
    // Keep Vite's event loop free to serve this actual system WebView.
    const result=await new Promise((resolve,reject)=>{
      const child=spawn(binary,[origin+'/__writing-tools']);let output=''
      child.stdout.on('data',data=>output+=data);child.stderr.on('data',data=>output+=data)
      const timer=setTimeout(()=>{child.kill();reject(new Error('Native WebView test timed out: '+output))},35000)
      child.on('error',reject);child.on('exit',code=>{clearTimeout(timer);resolve({code,output})})
    })
    assert.equal(result.code,0,result.output);console.log(result.output.trim())
  }finally{await rm(directory,{recursive:true,force:true})}
})

test('no browser errors',()=>assert.deepEqual(errors,[]))
