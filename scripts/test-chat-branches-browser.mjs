// Real Vue composers with mocked desktop/provider I/O. No external requests.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, context, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke = (command, args) => window.__test?.invoke(command, args) ?? Promise.resolve(null);
    export const convertFileSrc = path => path; export const isTauri = () => false;`,
  '@tauri-apps/api/event': `export const listen = async (name, callback) => {
    if (!window.__test) return () => {};
    const callbacks = window.__test.listeners.get(name) || new Set();
    window.__test.listeners.set(name, callbacks); callbacks.add(callback); return () => callbacks.delete(callback);
  }; export const emit = async (name, payload) => window.__test?.emit(name, payload);
     export const emitTo = async (target, name, payload) => window.__test?.emit(name, payload);`,
  '@tauri-apps/api/window': `export const getCurrentWindow = () => ({ label: 'main',
    onResized: async () => () => {}, onFocusChanged: async () => () => {} });
    export class Window { static async getByLabel() { return null; } }`,
  '@tauri-apps/api/webviewWindow': `export const getCurrentWebviewWindow = () => ({ label: 'main' });
    export class WebviewWindow { static async getByLabel() { return null; } }`,
}
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
  plugins: [{ name: 'mock-desktop', enforce: 'pre',
    resolveId: source => source in mocks ? '\0mock:' + source : null,
    load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null,
  }, vue()], optimizeDeps: { entries: ['src/components/tabs/AiTab.vue', 'src/components/LibraryChat.vue'] },
  server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__branch-test')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><html><meta charset="utf-8"><body><div id="app"></div>
      <style>*{box-sizing:border-box;margin:0;padding:0}#app{height:100vh}.test-panel{height:100vh;width:500px}.lc-root{height:100vh}</style>
      <script type="module">
      import { createApp, h, ref } from 'vue';
      import { createPinia } from 'pinia';
      import { createI18n } from 'vue-i18n';
      import { addCollection } from '@iconify/vue';
      import fluent from '@iconify-json/fluent/icons.json';
      import AiTab from '/src/components/tabs/AiTab.vue';
      import LibraryChat from '/src/components/LibraryChat.vue';
      import { useAiStore } from '/src/stores/ai.ts';
      import { useSettingsStore } from '/src/stores/settings.ts';
      import { zh, en } from '/src/i18n/locales.ts';
      import '/src/assets/main.css'; import '/src/assets/themes.css';
      addCollection(fluent);
      const mode = new URLSearchParams(location.search).get('mode'), slug = ref('paper');
      const ts = '2026-10-15T00:00:00Z';
      const image = {slug:'paper',page:2,file:'figure.png',caption:'Retained figure'};
      const steps = [{tool:'view_paper_page',args:'page 2',argsJson:'{"slug":"paper","page":2}',ok:true,preview:'Evidence from the paper',images:[image]}];
      const fixture = {id:'original',slug:'paper',title:'原始研究问题',createdAt:ts,updatedAt:ts,selectedPaperSlugs:['paper'],
        usageRuns:{old:{id:'old',answerId:'answer-1',providerId:'provider',modelId:'test-vision',costUsd:99}},
        contextCheckpoint:{throughMessageId:'user-3',messages:[{role:'user',content:'obsolete checkpoint'}]},
        contextStates:{'provider::test-vision':{checkpoint:{messages:[{role:'user',content:'obsolete checkpoint'}]}}},
        messages:[],nodes:[]};
      for(let turn=1;turn<=3;turn++) {
        const user={id:'user-'+turn,role:'user',content:'Question '+turn,createdAt:ts,
          attachments:turn===1?[{id:'att',name:'reference.pdf',type:'pdf',dataUrl:'data:application/pdf;base64,JVBERi0='}]:undefined};
        const base={id:'answer-'+turn,content:'Answer '+turn,createdAt:ts,providerId:'provider',providerName:'Example',modelId:'test-vision',modelName:'Test vision',
          inputTokens:10,outputTokens:5,costUsd:99};
        const alternate={...base,id:base.id+'-selected',content:'Chosen answer '+turn,steps,agentSteps:steps,reasoningContent:'Preserved thinking'};
        fixture.nodes.push(user,{id:'group-'+turn,role:'assistantGroup',promptId:user.id,createdAt:ts,answers:[alternate,base]});
        fixture.messages.push(user,{...base,role:'assistant',variants:[base,alternate],activeVariantId:alternate.id});
      }
      if(new URLSearchParams(location.search).has('metadata')) {
        fixture.source='metadataExtraction';fixture.title='AI 元数据提取';
        fixture.nodes[0].content='Extract academic paper metadata from this paper';
      }
      let disk=JSON.parse(sessionStorage.getItem('branch-disk')||'null')||[fixture];
      const saveDisk=()=>sessionStorage.setItem('branch-disk',JSON.stringify(disk));
      const test = window.__test = { requests: [], calls: [], listeners: new Map(), saved: [], cancelAuto: true,
        emit(name, payload) { for (const cb of test.listeners.get(name) || []) cb({ payload }); },
        switchPaper: value => slug.value = value,
        finish(index, text='finished') { const r=test.requests[index]; r.finished=true; r.resolve(text); },
        invoke(command, args) {
          test.calls.push({command,args});
          if (command === 'get_current_library') return Promise.resolve(test.root || '/mock-library');
          if (test.root === '/other-library' && ['get_library_conversations','get_paper_ai_conversations'].includes(command)) return Promise.resolve([]);
          if (['get_library_conversations','get_paper_ai_conversations'].includes(command)) return Promise.resolve(JSON.parse(JSON.stringify(disk)));
          if (test.failSave && ['save_paper_ai_conversations','save_library_conversation'].includes(command) && (args.conversations?.some(c=>c.branch)||args.conversation?.branch)) return Promise.reject('Cannot write conversation');
          if (command === 'save_paper_ai_conversations') {disk=JSON.parse(JSON.stringify(args.conversations));saveDisk();}
          if (command === 'save_library_conversation') {disk=disk.filter(c=>c.id!==args.conversation.id);disk.push(JSON.parse(JSON.stringify(args.conversation)));saveDisk();}
          if (command === 'delete_library_conversation') {disk=disk.filter(c=>c.id!==args.id);saveDisk();}
          if (command === 'copy_chat_branch_images') {
            if(test.failCopy) return Promise.reject('Disk full');
            if(test.delayCopy) return new Promise(resolve=>test.releaseCopy=resolve);
          }
          if (command === 'read_chat_image') return Promise.resolve('data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=');
          if (command === 'list_papers') return Promise.resolve([{slug:'paper',title:'Example paper'}]);
          if (command === 'prepare_library_chat_context') return Promise.resolve({messages:args.messages,compacted:false});
          if (command === 'get_ai_settings') return Promise.resolve(JSON.parse(JSON.stringify(test.aiSettings)));
          if (command === 'get_paper_meta') return Promise.resolve({title:'Example paper',abstract:'A paper'});
          if (command.startsWith('save_')) test.saved.push({command,args:JSON.parse(JSON.stringify(args))});
          if (command === 'chat_with_library') return new Promise((resolve,reject) => test.requests.push({args,resolve,reject}));
          if (command === 'cancel_ai_request' && test.cancelAuto) {
            const r = test.requests.find(r => r.args.requestId === args.requestId);
            if (r && !r.finished) { r.finished=true; r.reject('cancelled'); }
          }
          return Promise.resolve(null);
        },
      };
      const app = createApp({setup(){return () => mode === 'library' ? h(LibraryChat)
        : h('div',{class:'test-panel'},[h(AiTab,{slug:slug.value,standalone:mode === 'standalone'})]);}});
      const pinia=createPinia(); app.use(pinia);
      const i18n=createI18n({legacy:false,locale:'zh',messages:{zh,en}}); app.use(i18n); test.setLocale=value=>i18n.global.locale.value=value;
      const ai=useAiStore(pinia); ai.loaded=true;
      ai.settings={default_provider_id:'provider',default_model_id:'test-vision',providers:[{
        id:'provider',name:'Example',enabled:true,kind:'custom',base_url:'https://example.test/v1',
        models:[{id:'test-vision',display_name:'Test vision',capabilities:['vision','tool_calling']}],
      }]};
      test.aiSettings=JSON.parse(JSON.stringify(ai.settings));
      ai.load=async()=>{};
      useSettingsStore(pinia).load=async()=>{};
      app.mount('#app'); test.mounted=true;
      </script></body></html>`))
  })
  await server.listen(); origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  context = await browser.newContext({ viewport: { width: 1100, height: 840 } })
  await context.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
  page = await context.newPage(); page.on('pageerror', e => { errors.push(e.message); console.error('Browser error:', e.message) })
})
after(async () => { await browser?.close(); await server?.close() })
async function open(mode,extra='') {
  await page.goto(`${origin}/__branch-test?mode=${mode}${extra}`)
  await page.evaluate(()=>{sessionStorage.clear();localStorage.clear()})
  await page.reload()
  await page.waitForFunction(() => window.__test?.mounted, null, { timeout: 10000 }).catch(async error => {
    console.error(await page.evaluate(() => ({ html: document.body.innerHTML.slice(0, 200), calls: window.__test?.calls, errors: window.__test?.errors })))
    throw error
  })
  await page.locator('.composer textarea').waitFor()
  if (mode==='standalone') await page.locator('.test-panel').evaluate(el=>el.style.width='100%')
  await page.locator('.branch-action').first().waitFor()
  await page.waitForFunction(() => window.__test.calls.some(c => c.command === 'list_papers' || c.command === 'get_paper_meta'))
}
const composer = () => page.locator('.composer textarea').last()
async function send(text, key = 'Enter') { await composer().fill(text); await composer().press(key) }
const count = n => page.waitForFunction(n => window.__test.requests.length === n, n)
const finish = (index, text) => page.evaluate(({index,text}) => window.__test.finish(index,text), {index,text})
const history = index => page.evaluate(index => window.__test.requests[index].args.messages, index)
const disk = () => page.evaluate(() => JSON.parse(sessionStorage.getItem('branch-disk') || '[]'))
const branchAt = async index => {
  const assistant=page.locator('.msg-row.assistant').nth(index)
  if(await assistant.count()) await assistant.hover()
  await page.locator('.branch-action').nth(index).click()
}
const expectBranch = () => page.locator('.branch-origin').waitFor()
async function showHistory(mode) {
  if(mode==='sidebar') await page.getByTitle('对话历史',{exact:true}).click()
}
const rows = mode => page.locator(mode==='library'?'.conv-item':mode==='sidebar'?'.history-panel .history-item':'.history-sidebar .history-item')

for (const mode of ['sidebar','standalone','library']) {
  test(`${mode}: forks exactly the chosen prefix with attachments, selected answers and tool evidence; follow-up and reload stay independent`, async () => {
    await open(mode)
    await composer().fill('Unsent draft')
    await branchAt(1)
    await expectBranch()
    assert.equal(await composer().inputValue(),'Unsent draft')
    let records=await disk(), child=records.find(c=>c.branch)
    assert.ok(child)
    assert.equal(child.branch.parentId,'original')
    assert.equal(child.branch.turn,2)
    assert.equal(child.contextCheckpoint,undefined)
    assert.equal(child.contextStates,undefined)
    assert.deepEqual(child.usageRuns,{})
    const messages=mode==='library'?child.messages:child.nodes
    assert.equal(messages.length,4)
    assert.equal(messages[0].attachments[0].name,'reference.pdf')
    assert.ok(!JSON.stringify(messages).includes('Question 3'))
    assert.ok(!JSON.stringify(messages).includes('Answer 1'))
    assert.ok(JSON.stringify(messages).includes('Chosen answer 1'))
    assert.ok(JSON.stringify(messages).includes('Evidence from the paper'))
    assert.ok(!JSON.stringify(messages).includes('displayContent'))
    const original=records.find(c=>c.id==='original')
    const sourceIds=new Set((mode==='library'?original.messages:original.nodes).map(m=>m.id))
    assert.ok(messages.every(m=>!sourceIds.has(m.id)))
    if(mode!=='library') {
      assert.equal(messages[1].answers.length,1)
      assert.equal(messages[1].promptId,messages[0].id)
      assert.notEqual(messages[1].answers[0].id,'answer-1-selected')
    }
    assert.equal(await page.locator('.branch-origin button').textContent().then(s=>s.trim()),'原始研究问题')
    const copied=await page.evaluate(()=>window.__test.calls.find(c=>c.command==='copy_chat_branch_images').args)
    assert.deepEqual(copied.files,['figure.png'])
    assert.equal(copied.targetId,child.id)
    await send('Explore another direction')
    await count(1)
    const sent=await history(0)
    assert.ok(sent.some(m=>m.role==='tool'&&m.content==='Evidence from the paper'))
    assert.ok(sent.some(m=>m.role==='assistant'&&m.content==='Chosen answer 2'))
    assert.ok(!JSON.stringify(sent).includes('Question 3'))
    assert.ok(JSON.stringify(sent).includes('argus-chat-image:'))
    assert.ok(!JSON.stringify(sent).includes('obsolete checkpoint'))
    await finish(0,'Independent continuation')
    await page.getByText('Independent continuation',{exact:true}).waitFor()
    records=await disk()
    assert.equal((mode==='library'?records.find(c=>c.id==='original').messages:records.find(c=>c.id==='original').nodes).length,6)
    await showHistory(mode)
    const list=rows(mode)
    assert.equal(await list.count(),2)
    assert.ok(!(await list.nth(0).getAttribute('class')).includes('is-branch'))
    assert.ok((await list.nth(1).getAttribute('class')).includes('is-branch'))
    assert.equal(await list.nth(1).evaluate(el=>parseInt(getComputedStyle(el).marginLeft)),14)
    if(mode==='sidebar') await page.locator('.history-layer').click({position:{x:1,y:10}})
    await page.reload()
    await page.locator('.branch-action').first().waitFor()
    if(mode==='standalone') await page.locator('.test-panel').evaluate(el=>el.style.width='100%')
    if(mode==='library') {
      await rows(mode).filter({hasText:'分支'}).click()
    }
    await expectBranch()
    await page.getByText('Independent continuation',{exact:true}).waitFor()
    await page.locator('.branch-origin button').click()
    assert.equal(await page.locator('.branch-origin').count(),0)
    await page.getByText('Question 3',{exact:true}).waitFor()
  })

  test(`${mode}: nested branches stay beneath their parent and survive deletion of the original`,async()=>{
    await open(mode)
    await branchAt(0);await expectBranch()
    await branchAt(0);await expectBranch()
    await showHistory(mode)
    const list=rows(mode)
    assert.equal(await list.count(),3)
    assert.deepEqual(await list.evaluateAll(els=>els.map(el=>parseInt(getComputedStyle(el).marginLeft)||0)),[0,14,28])
    const branchParentId=(await disk()).find(c=>c.branch?.parentId!=='original'&&c.branch)?.branch.parentId
    assert.ok(branchParentId)
    await list.nth(1).hover()
    await list.nth(1).locator(mode === 'library' ? '.conv-del-btn' : '.history-delete').click()
    assert.equal(await list.count(),2)
    if(mode==='sidebar') await page.locator('.history-layer').click({position:{x:1,y:10}})
    await page.getByText(/原对话已删除/).waitFor()
    await send('Still usable after parent deletion');await count(1)
    assert.ok(JSON.stringify(await history(0)).includes('Chosen answer 1'))
    await finish(0,'Survived deletion')
  })

  test(`${mode}: a failed image copy leaves the original and draft intact`,async()=>{
    await open(mode)
    await composer().fill('Keep this draft')
    await page.evaluate(()=>window.__test.failCopy=true)
    await branchAt(0)
    await page.getByRole('alert').waitFor()
    assert.match(await page.getByRole('alert').textContent(),/Disk full/)
    assert.equal(await page.locator('.branch-origin').count(),0)
    assert.equal(await composer().inputValue(),'Keep this draft')
    assert.equal(await page.getByText('Question 3',{exact:true}).count(),1)
    await page.evaluate(()=>window.__test.failCopy=false)
    await branchAt(0);await expectBranch()
  })

  test(`${mode}: nested branches fold independently, remember their state and reveal a newly created branch`, async () => {
    await open(mode)
    await branchAt(0); await expectBranch()
    await branchAt(0); await expectBranch()
    await showHistory(mode)
    const list = rows(mode)
    assert.equal(await list.count(), 3)
    if (process.env.ARGUS_TEST_OUTPUT_DIR) {
      await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
      await page.evaluate(() => document.activeElement?.blur())
      await page.mouse.move(1000, 20)
      await page.screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/branch-expanded-' + mode + '.png' })
    }
    const origin = await page.locator('.branch-origin').textContent()
    const saves = await page.evaluate(() => window.__test.calls.filter(c => c.command.startsWith('save_')).length)
    // Aim at the idle arrow, hover until Delete is fully visible, then click
    // those original coordinates. Tracking the locator would hide this bug.
    await page.evaluate(() => document.activeElement?.blur())
    await page.mouse.move(1000, 20)
    const arrow = list.nth(1).locator('.chat-branch-toggle')
    const beforeHover = await arrow.boundingBox()
    const target = { x: beforeHover.x + beforeHover.width / 2, y: beforeHover.y + beforeHover.height / 2 }
    await page.mouse.move(target.x, target.y)
    await list.nth(1).locator(mode === 'library' ? '.conv-del-btn' : '.history-delete').evaluate(async el => {
      void el.offsetWidth
      await Promise.all(el.getAnimations().map(animation => animation.finished))
    })
    const afterHover = await arrow.boundingBox()
    assert(Math.abs(beforeHover.x - afterHover.x) < 0.5, 'hovering must not move the arrow under the pointer')
    await page.mouse.click(target.x, target.y)
    assert.equal(await list.count(), 2)
    assert.equal(await list.nth(1).locator('.history-branch-count').textContent(), '1 个分支')
    await list.nth(0).locator('.chat-branch-toggle').click()
    assert.equal(await list.count(), 1)
    assert.equal(await list.nth(0).locator('.history-branch-count').textContent(), '2 个分支')
    if (process.env.ARGUS_TEST_OUTPUT_DIR) {
      await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
      await page.evaluate(() => document.activeElement?.blur())
      await page.mouse.move(1000, 20)
      await page.screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/branch-collapsed-' + mode + '.png' })
      if (mode === 'standalone') {
        await page.evaluate(() => document.documentElement.dataset.theme = 'dark')
        await page.screenshot({ path: process.env.ARGUS_TEST_OUTPUT_DIR + '/branch-collapsed-dark.png' })
      }
    }
    assert.equal(await page.locator('.branch-origin').textContent(), origin)
    assert.equal(await page.evaluate(() => window.__test.calls.filter(c => c.command.startsWith('save_')).length), saves)
    await page.reload()
    await page.locator('.branch-action').first().waitFor()
    if (mode === 'standalone') await page.locator('.test-panel').evaluate(el => el.style.width = '100%')
    await showHistory(mode)
    assert.equal(await list.count(), 1)
    const rootToggle = list.nth(0).locator('.chat-branch-toggle')
    assert.equal(await rootToggle.getAttribute('aria-expanded'), 'false')
    await rootToggle.focus(); await rootToggle.press('Space')
    assert.equal(await list.count(), 2)
    assert.equal(await list.nth(1).locator('.chat-branch-toggle').getAttribute('aria-expanded'), 'false')
    await list.nth(1).locator('.chat-branch-toggle').click()
    assert.equal(await list.count(), 3)
    await rootToggle.click()
    await list.nth(0).click()
    // Fork from a collapsed source; the new child should be immediately visible.
    await branchAt(0); await expectBranch(); await showHistory(mode)
    assert.equal(await list.count(), 4)
    assert.equal(await list.nth(0).locator('.chat-branch-toggle').getAttribute('aria-expanded'), 'true')
  })

  test(`${mode}: branching an earlier reply does not stop the original generation`,async()=>{
    await open(mode)
    await send('Original background question');await count(1)
    await branchAt(0);await expectBranch()
    assert.equal(await page.evaluate(()=>window.__test.calls.filter(c=>c.command==='cancel_ai_request').length),0)
    await send('Parallel branch question');await count(2)
    await finish(1,'Branch finished')
    await finish(0,'Original finished')
    await page.getByText('Branch finished',{exact:true}).waitFor()
    await page.locator('.branch-origin button').click()
    await page.getByText('Original finished',{exact:true}).waitFor()
  })
}

test('narrow sidebar and dark standalone keep branch provenance and controls visible',async()=>{
  await open('sidebar')
  await page.locator('.test-panel').evaluate(el=>el.style.width='320px')
  await branchAt(0);await expectBranch()
  assert.ok(await page.locator('.branch-origin button').isVisible())
  await showHistory('sidebar')
  assert.equal(await rows('sidebar').count(),2)
  if(process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true})
    await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/branch-sidebar.png'})
  }
  await open('standalone')
  await page.evaluate(()=>document.documentElement.setAttribute('data-theme','dark'))
  await branchAt(1);await expectBranch()
  assert.ok(await rows('standalone').nth(1).isVisible())
  await page.locator('.messages').evaluate(el=>el.scrollTop=0)
  const darkColors=await page.locator('.history-sidebar').evaluate(el=>({bg:getComputedStyle(el).backgroundColor,text:getComputedStyle(el.querySelector('.history-title')).color}))
  assert.notEqual(darkColors.bg,'rgb(245, 245, 247)')
  assert.notEqual(darkColors.bg,darkColors.text)
  if(process.env.ARGUS_TEST_OUTPUT_DIR) await page.screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/branch-standalone.png'})
  assert.deepEqual(errors,[])
})

for (const mode of ['sidebar','standalone','library']) {
  test(mode+': save failures are visible and do not leave a phantom branch',async()=>{
    await open(mode)
    await page.evaluate(()=>window.__test.failSave=true)
    await branchAt(0)
    await page.getByRole('alert').waitFor()
    assert.match(await page.getByRole('alert').textContent(),/Cannot write conversation/)
    await showHistory(mode)
    assert.equal(await rows(mode).count(),1)
  })
  test(mode+': a delayed branch cannot follow a library or paper switch',async()=>{
    await open(mode)
    await page.evaluate(()=>window.__test.delayCopy=true)
    await branchAt(0)
    await page.waitForFunction(()=>!!window.__test.releaseCopy)
    if(mode==='library') {
      await page.evaluate(()=>{window.__test.root='/other-library';window.__test.emit('library-changed',{})})
      await page.waitForFunction(()=>window.__test.calls.filter(c=>c.command==='get_library_conversations').length>=2)
    } else {
      await page.evaluate(()=>window.__test.switchPaper('other-paper'))
      await page.waitForFunction(()=>window.__test.calls.some(c=>c.command==='get_paper_meta'&&c.args.slug==='other-paper'))
    }
    await page.evaluate(()=>window.__test.releaseCopy())
    await page.waitForFunction(()=>![...document.querySelectorAll('.branch-action')].some(b=>b.disabled))
    assert.equal(await page.locator('.branch-origin').count(),0)
    assert.ok(!(await disk()).some(c=>c.branch))
  })
}

test('English branch controls and provenance are localized',async()=>{
  await open('library')
  await page.evaluate(()=>window.__test.setLocale('en'))
  await page.getByRole('button',{name:'Branch in a new conversation',exact:true}).first().waitFor()
  await branchAt(0);await expectBranch()
  await page.getByText('Includes context through turn 1 of the original conversation',{exact:true}).waitFor()
})

for(const mode of ['sidebar','standalone','library']) {
  test(mode+': returning to the parent scrolls to the branch point in a long conversation',async()=>{
    await open(mode)
    // Grow the original after the branch point using a real generated answer.
    await send('Explain at length');await count(1)
    await finish(0,'Later detail. '.repeat(3000))
    await page.getByText('Explain at length',{exact:true}).waitFor()
    await branchAt(0);await expectBranch()
    await page.locator('.messages').evaluate(el=>el.scrollTop=0)
    await page.locator('.branch-origin button').click()
    const attr=mode==='library'?'data-msg-id':'data-node-id';
    const id=mode==='library'?'answer-1':'group-1';
    await page.waitForFunction(({attr,id})=>{
      const container=document.querySelector('.messages');
      const target=container.querySelector('['+attr+'="'+id+'"]');
      if(!target) return false;
      const delta=target.getBoundingClientRect().top-container.getBoundingClientRect().top;
      return delta>=0&&delta<35;
    },{attr,id})
  })
}

test('a branch of metadata extraction becomes a normal chat with its own title and model controls',async()=>{
  await open('standalone','&metadata=1')
  assert.equal(await page.locator('.floating-model-picker').count(),0)
  await branchAt(0);await expectBranch()
  assert.equal(await page.locator('.floating-model-picker').count(),1)
  assert.match(await page.locator('.header-title').textContent(),/分支 1/)
  await send('Explain the method');await count(1)
  assert.ok(JSON.stringify(await history(0)).includes('Extract academic paper metadata'))
  await finish(0,'A normal conversation')
})
