// Real sidebar/tree/Pinia stores with a mock library. No real library writes.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const errors = []
const mocks = {
  '@tauri-apps/api/core': `export const invoke=(name,args)=>window.__test.invoke(name,args);export const convertFileSrc=p=>p;export const isTauri=()=>false;`,
  '@tauri-apps/api/event': `export const listen=async(name,handler)=>{const set=window.__test.listeners.get(name)||new Set();window.__test.listeners.set(name,set);set.add(handler);return()=>set.delete(handler)};export const emit=async(name,payload)=>window.__test.emit(name,payload);`,
}
before(async () => {
  server = await createServer({ configFile: false, root: new URL('../', import.meta.url).pathname, appType: 'custom',
    plugins: [{ name: 'mock-collection-pins', enforce: 'pre', resolveId: s => s in mocks ? '\0mock:' + s : null,
      load: id => id.startsWith('\0mock:') ? mocks[id.slice(6)] : null }, vue()],
    optimizeDeps: { entries: ['src/components/LeftSidebar.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error' })
  server.middlewares.use(async (req, res, next) => {
    if (!req.url.startsWith('/__pins-test')) return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url, `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><div id="app"></div>
      <style>*{box-sizing:border-box;margin:0;padding:0}html,body,#app{height:100%}.left-sidebar{width:280px;height:100vh}</style><script type="module">
      import{createApp,h}from'vue';import{createPinia}from'pinia';import{createI18n}from'vue-i18n';
      import{addCollection}from'@iconify/vue';import fluent from'@iconify-json/fluent/icons.json';
      import LeftSidebar from'/src/components/LeftSidebar.vue';import{useLibraryStore}from'/src/stores/library.ts';
      import{useCollectionsStore}from'/src/stores/collections.ts';import{useSelectionStore}from'/src/stores/selection.ts';
      import{zh,en}from'/src/i18n/locales.ts';import'/src/assets/main.css';import'/src/assets/themes.css';addCollection(fluent);
      const initial={collections:[
        {id:'misc',name:'00-Misc',emoji:'📁'},
        {id:'ai',name:'01-AI经典',emoji:'🧠'},
        {id:'nus',name:'02-NUS',emoji:'🧪'},
        {id:'course',name:'CS6215 Advanced Topics',parent_id:'nus'},
        {id:'project',name:'MyPre & Project',parent_id:'course'},
        {id:'read',name:'20-稍后阅读',parent_id:'misc'},
        {id:'paper-group',name:'论文与实验',parent_id:'project'}
      ],assignments:[{paper_id:'p1',collection_id:'project'},{paper_id:'p2',collection_id:'paper-group'}]};
      const t=window.__test={root:'/mock-library',listeners:new Map(),calls:[],failPin:false,delayRead:false,delayPin:false,reads:[],
        disk:JSON.parse(localStorage.getItem('mock-collections')||'null')||initial,
        persist(){localStorage.setItem('mock-collections',JSON.stringify(t.disk))},
        emit(name,payload){for(const handler of t.listeners.get(name)||[])handler({payload})},
        releaseReads(){for(const release of t.reads.splice(0))release()},
        invoke(name,args){t.calls.push({name,args});if(args?.expectedRoot&&args.expectedRoot!==t.root)return Promise.reject('Library changed');
          if(name==='get_collections'){const snapshot=structuredClone(t.disk);if(t.delayRead)return new Promise(resolve=>t.reads.push(()=>resolve(snapshot)));return Promise.resolve(snapshot)}
          if(name==='list_canvases')return Promise.resolve([]);
          if(name==='set_collection_pinned'){
            if(t.failPin)return Promise.reject('Cannot write mock library');
            const commit=()=>{t.disk.collections.find(c=>c.id===args.id).pinned=args.pinned;t.persist();t.emit('collections-changed',{root:t.root})};
            if(t.delayPin)return new Promise(resolve=>t.releasePin=()=>{commit();resolve()});commit();return Promise.resolve();
          }
          return Promise.resolve(null);
        }};
      t.persist();const pinia=createPinia(),i18n=createI18n({legacy:false,locale:'zh',messages:{zh,en}});
      const app=createApp({render:()=>h(LeftSidebar)});app.use(pinia);app.use(i18n);
      t.library=useLibraryStore(pinia);t.library.currentPath=t.root;t.library.papers=[{id:'p1',slug:'paper-a',tags:[]},{id:'p2',slug:'paper-b',tags:[]}];
      t.collections=useCollectionsStore(pinia);t.selection=useSelectionStore(pinia);t.locale=value=>i18n.global.locale.value=value;
      t.unmount=()=>app.unmount();app.mount('#app');t.mounted=true;
      </script>`))
  })
  await server.listen(); origin = 'http://127.0.0.1:' + server.httpServer.address().port
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 650, height: 780 } })
  await page.route('**/*', r => r.request().url().startsWith(origin) ? r.continue() : r.abort())
  page.on('pageerror', e => errors.push(e.message))
})
after(async () => { await browser?.close(); await server?.close() })
async function open() {
  await page.goto(origin + '/__pins-test')
  await page.evaluate(() => localStorage.clear()); await page.reload()
  await page.locator('.coll-item[data-collection-id="nus"]').waitFor()
}
const tree = id => page.locator('.coll-item[data-collection-id="' + id + '"]')
const shortcut = id => page.locator('.pinned-collection-row[data-collection-id="' + id + '"]')
async function expandToProject() {
  await tree('nus').locator('.expand-btn').click()
  await tree('course').locator('.expand-btn').click()
}

test('nested collection pins create persistent shortcuts and retain the original tree and paper membership', async () => {
  await open(); await expandToProject()
  await tree('project').click({ button: 'right' })
  await page.getByRole('button', { name: '置顶到快捷区', exact: true }).click()
  await shortcut('project').waitFor()
  assert.equal(await tree('project').locator('.collection-pin-btn').getAttribute('aria-pressed'), 'true')
  assert.equal(await shortcut('project').locator('.shortcut-path').textContent(), '02-NUS / CS6215 Advanced Topics')
  assert.equal(await shortcut('project').locator('.shortcut-count').textContent(), '2')
  assert.equal(await page.evaluate(() => window.__test.collections.file.assignments.length), 2)
  await page.reload(); await shortcut('project').waitFor()
  // A deep shortcut still works with its ancestors collapsed.
  await tree('nus').locator('.expand-btn').click()
  assert.equal(await tree('project').count(), 0)
  await shortcut('project').locator('.pinned-collection-open').click()
  await tree('project').waitFor()
  assert.equal(await page.evaluate(() => window.__test.selection.activeCollectionId), 'project')
  assert.equal(await shortcut('project').getAttribute('class'), 'pinned-collection-row active')
  await shortcut('project').locator('.shortcut-unpin').click()
  await shortcut('project').waitFor({ state: 'detached' })
  assert.equal(await page.evaluate(() => window.__test.selection.activeCollectionId), 'project')
  assert.equal(await tree('project').locator('.collection-pin-btn').getAttribute('aria-pressed'), 'false')
})

test('top-level pins support keyboard actions; rejected saves preserve the shortcut and show an error', async () => {
  await open()
  const pin = tree('ai').locator('.collection-pin-btn')
  await pin.focus(); await pin.press('Enter'); await shortcut('ai').waitFor()
  await page.evaluate(() => window.__test.failPin = true)
  await shortcut('ai').locator('.shortcut-unpin').click()
  await page.getByRole('alert').waitFor()
  assert.equal(await shortcut('ai').count(), 1)
  assert.equal(await tree('ai').locator('.collection-pin-btn').getAttribute('aria-pressed'), 'true')
  await page.evaluate(() => window.__test.failPin = false)
  await pin.click(); await shortcut('ai').waitFor({ state: 'detached' })
  assert.equal(await page.getByRole('alert').count(), 0)
  assert.equal(await page.locator('.pinned-collections').count(), 0)
})

test('stale reads and repeated clicks cannot undo a saved pin; foreign-library notifications are ignored', async () => {
  await open()
  await page.evaluate(() => { const t = window.__test; t.delayRead = true; void t.collections.load() })
  await tree('misc').locator('.collection-pin-btn').click()
  await shortcut('misc').waitFor()
  await page.evaluate(() => { window.__test.delayRead = false; window.__test.releaseReads() })
  assert.equal(await shortcut('misc').count(), 1)
  await page.evaluate(() => { const t = window.__test; t.calls.length = 0; t.emit('collections-changed', { root: '/other-library' }) })
  assert.equal(await page.evaluate(() => window.__test.calls.length), 0)
  await page.evaluate(() => window.__test.delayPin = true)
  await shortcut('misc').locator('.shortcut-unpin').click()
  assert.equal(await shortcut('misc').locator('.shortcut-unpin').isDisabled(), true)
  await page.evaluate(() => window.__test.releasePin())
  await shortcut('misc').waitFor({ state: 'detached' })
  assert.equal(await page.evaluate(() => window.__test.calls.filter(c => c.name === 'set_collection_pinned').length), 1)
})

test('a delayed read from the previous library cannot replace the current library shortcuts', async () => {
  await open()
  await tree('ai').locator('.collection-pin-btn').click(); await shortcut('ai').waitFor()
  await page.evaluate(async () => {
    const t = window.__test; t.delayRead = true; void t.collections.load()
    t.root = '/second-library'; t.library.currentPath = t.root; t.delayRead = false
    t.disk = { collections: [{ id: 'new', name: '第二个文献库', pinned: true }], assignments: [] }
    await t.collections.load(); t.releaseReads()
  })
  await shortcut('new').waitFor()
  assert.equal(await shortcut('ai').count(), 0)
  assert.equal(await page.evaluate(() => window.__test.calls.find(c => c.name === 'set_collection_pinned').args.expectedRoot), '/mock-library')
})

test('a canonical reload follows renames, parent moves and deletions without orphan shortcuts', async () => {
  await open(); await expandToProject()
  await tree('project').locator('.collection-pin-btn').click(); await shortcut('project').waitFor()
  await page.evaluate(() => {
    const t = window.__test, col = t.disk.collections.find(c => c.id === 'project')
    col.name = '重命名后的分类'; col.parent_id = 'misc'; t.emit('collections-changed', { root: t.root })
  })
  await page.waitForFunction(() => document.querySelector('.shortcut-name')?.textContent === '重命名后的分类')
  assert.equal(await shortcut('project').locator('.shortcut-path').textContent(), '00-Misc')
  await page.evaluate(() => { const t = window.__test; t.disk.collections = t.disk.collections.filter(c => c.id !== 'project'); window.dispatchEvent(new Event('focus')) })
  await shortcut('project').waitFor({ state: 'detached' })
})

test('compact light/dark/English layouts keep the shortcuts scrollable and release the new listeners', async () => {
  await open()
  await page.evaluate(() => { const t = window.__test; for (const c of t.disk.collections) c.pinned = true; t.emit('collections-changed', { root: t.root }) })
  await page.waitForFunction(() => document.querySelectorAll('.pinned-collection-row').length === 7)
  await page.setViewportSize({ width: 440, height: 600 })
  await page.evaluate(() => document.querySelector('.left-sidebar').style.width = '220px')
  const panel = await page.locator('.pinned-collections-list').evaluate(el => ({ h: el.clientHeight, sh: el.scrollHeight, w: el.clientWidth, sw: el.scrollWidth }))
  assert(panel.sh > panel.h); assert.equal(panel.sw, panel.w)
  assert((await page.locator('.library-main-scroll').boundingBox()).height > 80)
  const output = process.env.ARGUS_TEST_OUTPUT_DIR
  if (output) { await mkdir(output, { recursive: true }); await page.screenshot({ path: output + '/collection-pins-light.png' }) }
  await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; window.__test.locale('en') })
  await page.getByRole('region', { name: 'Pinned collections' }).waitFor()
  if (output) await page.screenshot({ path: output + '/collection-pins-dark.png' })
  await page.evaluate(() => window.__test.unmount())
  assert.equal(await page.evaluate(() => window.__test.listeners.get('collections-changed').size), 0)
  assert.deepEqual(errors, [])
})
