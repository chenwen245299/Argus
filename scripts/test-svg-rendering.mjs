// Real MarkdownBody/renderMarkdown/DOMPurify + image decoding, with a mock
// clipboard. No real library, provider, system clipboard or remote requests.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'
import vue from '@vitejs/plugin-vue'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
const remoteRequests = []
before(async () => {
  server = await createServer({
    configFile: false, root: new URL('../', import.meta.url).pathname,
    appType: 'custom', plugins: [vue()],
    cacheDir: 'node_modules/.vite-svg-test',
    optimizeDeps: { entries: ['src/components/MarkdownBody.vue'] },
    server: { host: '127.0.0.1', port: 0 }, logLevel: 'error',
  })
  server.middlewares.use(async (req, res, next) => {
    if (req.url !== '/__svg-test') return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(await server.transformIndexHtml('/__svg-test.html', `<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,">
      <style>body{font:16px sans-serif}#answer{width:850px;max-width:calc(100vw - 48px);margin:24px}</style>
      <div id="answer" class="markdown-body"></div><p id="outside">Outside SVG</p>
      <script type="module">
        import { createApp, ref, h } from 'vue';
        import MarkdownBody from '/src/components/MarkdownBody.vue';
        import { renderMarkdown } from '/src/utils/renderMarkdown.ts';
        import { svgStringToPngBlob, normalizeSvgDimensions } from '/src/utils/svgToPng.ts';
        import { fragmentToMarkdown } from '/src/utils/copyAsMarkdown.ts';
        import '/src/assets/main.css'; import '/src/assets/themes.css';
        const content = ref(''), streaming = ref(false);
        createApp({render: () => h(MarkdownBody, {content:content.value, streaming:streaming.value})}).mount('#answer');
        window.setContent = (text, live=false) => {content.value=text; streaming.value=live};
        Object.assign(window, {renderMarkdown, svgStringToPngBlob, normalizeSvgDimensions, fragmentToMarkdown});
        Object.defineProperty(navigator, 'clipboard', {configurable:true,value:{
          write: async items => {window.copiedPng = await items[0].getType('image/png')},
          writeText: async text => {window.copiedText = text}
        }});
      </script>`))
  })
  await server.listen()
  origin = `http://127.0.0.1:${server.httpServer.address().port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 1000, height: 900 } })
  page.on('pageerror', error => console.error('Browser error:', error.message))
  page.on('console', message => { if (message.type()==='error') console.error('Browser console:',message.text()) })
  await page.route('**/*', route => {
    const url = route.request().url()
    if (url.startsWith(origin) || url.startsWith('data:') || url.startsWith('blob:')) return route.continue()
    remoteRequests.push(url)
    return route.abort()
  })
  await page.goto(`${origin}/__svg-test`)
  await page.waitForFunction(() => !!window.setContent, null, { timeout: 30000 })
})
after(async () => { await browser?.close(); await server?.close() })

// This indentation + blank line used to turn the entire drawing after the
// heading into a Markdown code block inside the SVG (only the title painted).
const drawing = `<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="auto" viewBox="0 0 800 360">
  <text x="20" y="36" font-size="24">骨架图 + 随机游走</text>

    <defs>
        <linearGradient id="paint"><stop stop-color="#2563eb"/><stop offset="1" stop-color="#2563eb"/></linearGradient>
        <marker id="arrow" markerWidth="10" markerHeight="10" refX="5" refY="3" orient="auto"><path d="M0 0 L6 3 L0 6Z" fill="#111"/></marker>
        <circle id="node" r="24" fill="#22c55e"/>
    </defs>

    <g>
        <rect x="20" y="80" width="180" height="120" fill="url(#paint)"/>
        <text x="30" y="140" fill="white">节点 A</text>
        <use href="#node" x="400" y="140"/>
        <path d="M220 140 L360 140" stroke="#111" stroke-width="3" marker-end="url(#arrow)"/>
    </g>

    <svg x="480" y="80" width="180" height="120" viewBox="0 0 180 120"><rect width="180" height="120" fill="#f97316"/></svg>
    <text x="20" y="300">价格 $10；原样保留 \`node\`</text>
</svg>`
const fence = (source, lang='svg', delimiter='```') => `${delimiter}${lang}\n${source}\n${delimiter}`

async function render(source, streaming=false) {
  await page.evaluate(([source, streaming]) => window.setContent(source, streaming), [source, streaming])
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(resolve)))
  await page.evaluate(async () => { await Promise.all([...document.querySelectorAll('.svg-preview-image')].map(img => img.decode())) })
}
async function pixels(selector='.svg-preview-image', point=[60, 100]) {
  return page.locator(selector).first().evaluate((img, [x,y]) => {
    const canvas = document.createElement('canvas'); canvas.width=img.naturalWidth; canvas.height=img.naturalHeight
    const ctx=canvas.getContext('2d'); ctx.drawImage(img,0,0)
    return {width:img.naturalWidth,height:img.naturalHeight,rgba:[...ctx.getImageData(x,y,1,1).data],
      painted:[...ctx.getImageData(0,60,canvas.width,canvas.height-60).data].filter((v,i)=>i%4===3 && v>0).length}
  },point)
}

test('raw SVG survives blank lines, indentation, nested SVG, gradients, markers and local use', async () => {
  await render(`前文\n\n${drawing}\n\n后文 $x_i$`)
  assert.equal(await page.locator('.svg-preview-image').count(),1)
  const result=await pixels()
  assert.deepEqual([result.width,result.height],[800,360])
  assert.deepEqual(result.rgba,[37,99,235,255])
  assert.ok(result.painted>42000)
  assert.deepEqual((await pixels('.svg-preview-image',[400,140])).rgba,[34,197,94,255])
  assert.deepEqual((await pixels('.svg-preview-image',[500,100])).rgba,[249,115,22,255])
  assert.equal(await page.locator('.katex').count(),1)
  const bounds=await page.locator('.svg-preview-image').boundingBox()
  assert.ok(bounds.height<400)
  assert.ok((await page.locator('#answer').innerText()).includes('后文'))
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR,{recursive:true})
    await page.locator('#answer').screenshot({path:`${process.env.ARGUS_TEST_OUTPUT_DIR}/svg-fixed.png`})
  }
})

test('SVG fences, alternate language labels and tildes use the same preview without affecting math', async () => {
  for (const [lang, delimiter] of [['svg','```'],['SVG title','```'],['xml','```'],['html','~~~'],['','````']]) {
    await render(fence(drawing,lang,delimiter)+'\n\n$x_i$')
    assert.deepEqual((await pixels()).rgba,[37,99,235,255],lang)
    assert.equal(await page.locator('.katex').count(),1)
  }
})

test('source toggle and selection copy retain the complete original SVG', async () => {
  await render(drawing)
  assert.equal(await page.locator('.svg-source-pre').isVisible(),false)
  await page.locator('[data-md-action="toggle-svg-source"]').click()
  assert.equal(await page.locator('.svg-source-pre').isVisible(),true)
  assert.equal(await page.locator('.svg-source-pre code').textContent(),drawing)
  const copied=await page.evaluate(() => {
    const f=document.createDocumentFragment(); f.append(document.querySelector('#answer').cloneNode(true))
    return window.fragmentToMarkdown(f)
  })
  assert.equal(copied.trim(),fence(drawing))
  await page.locator('[data-md-action="toggle-svg-source"]').click()
  assert.equal(await page.locator('.svg-source-pre').isVisible(),false)
})

test('styles and reused definition IDs stay isolated across diagrams and surrounding chat', async () => {
  const diagram=color=>`<svg viewBox="0 0 80 40"><style>body { display:none } .node { fill:${color} }</style><defs><rect id="box" class="node" width="80" height="40"/></defs><use href="#box"/></svg>`
  await render(diagram('#2563eb')+'\n\n'+fence(diagram('#f97316')))
  const colors=await page.locator('.svg-preview-image').evaluateAll(async images => {
    return images.map(img=>{const c=document.createElement('canvas');c.width=80;c.height=40;const x=c.getContext('2d');x.drawImage(img,0,0);return [...x.getImageData(20,20,1,1).data]})
  })
  assert.deepEqual(colors,[[37,99,235,255],[249,115,22,255]])
  assert.equal(await page.locator('#outside').isVisible(),true)
  assert.equal(await page.locator('#answer style').count(),0)
})

test('copy image works from raw and fenced previews and produces a painted PNG', async () => {
  for (const source of [drawing,fence(drawing)]) {
    await render(source)
    await page.evaluate(()=>{window.copiedPng=null})
    await page.locator('[data-md-action="copy-svg-image"] svg path').click()
    await page.waitForFunction(()=>!!window.copiedPng)
    const result=await page.evaluate(async()=>{
      const blob=window.copiedPng, bitmap=await createImageBitmap(blob)
      const c=document.createElement('canvas');c.width=bitmap.width;c.height=bitmap.height
      const x=c.getContext('2d');x.drawImage(bitmap,0,0)
      return {type:blob.type,width:bitmap.width,height:bitmap.height,rgba:[...x.getImageData(60,100,1,1).data]}
    })
    assert.deepEqual(result,{type:'image/png',width:800,height:360,rgba:[37,99,235,255]})
  }
})

test('PNG sizing resolves percentage/auto, partial dimensions and nonzero viewBox origins, and rejects invalid XML', async () => {
  const results=await page.evaluate(async()=>{
    const sources=[
      '<svg width="100%" height="100%" viewBox="10 20 800 400"><rect x="10" y="20" width="800" height="400" fill="red"/></svg>',
      '<svg width="400px" viewBox="0 0 800 400"/>',
      '<svg height="200" viewBox="0 0 800 400"/>',
      '<svg width="300" height="150"/>',
      '<svg width="2in" height="1in"/>',
    ]
    const sizes=[]
    for(const source of sources){const b=await createImageBitmap(await window.svgStringToPngBlob(source));sizes.push([b.width,b.height]);b.close()}
    const huge=new DOMParser().parseFromString('<svg width="100000" height="100000"/>','image/svg+xml').documentElement
    const bound=window.normalizeSvgDimensions(huge)
    let invalid=false;try{await window.svgStringToPngBlob('<svg><rect></svg>')}catch{invalid=true}
    return {sizes,bound,invalid}
  })
  assert.deepEqual(results.sizes,[[800,400],[400,200],[400,200],[300,150],[192,96]])
  assert.ok(results.bound.width*results.bound.height<=16_000_000)
  assert.equal(results.invalid,true)
})

test('sanitization keeps scripts, HTML, event handlers and external references out of the preview', async () => {
  const source=`<svg viewBox="0 0 200 100" onload="window.svgInjected=1"><style>@import url(https://invalid.example/evil.css); body{display:none}</style><script>window.svgInjected=1</script><foreignObject><div xmlns="http://www.w3.org/1999/xhtml">HTML</div></foreignObject><image href="https://invalid.example/evil.svg"/><use href="https://invalid.example/evil.svg#box"/><rect width="200" height="100" fill="blue" onclick="window.svgInjected=1"/></svg>`
  await render(source)
  const svg=await page.locator('.svg-preview-image').evaluate(img=>decodeURIComponent(img.src.slice(img.src.indexOf(',')+1)))
  assert.ok(!/<(?:script|foreignObject)\b|onload=|onclick=|href="https:/i.test(svg))
  assert.equal(await page.evaluate(()=>window.svgInjected),undefined)
  assert.equal(await page.locator('#outside').isVisible(),true)
  assert.deepEqual(remoteRequests,[])
})

test('non-SVG code examples and inline code remain source, invalid SVG fences stay readable', async () => {
  await render(fence(drawing,'python')+'\n\n`<svg viewBox="0 0 1 1"></svg>`')
  assert.equal(await page.locator('.svg-preview-image').count(),0)
  assert.equal(await page.locator('pre code').textContent(),drawing)
  assert.equal(await page.locator('.md-inline-code').textContent(),'<svg viewBox="0 0 1 1"></svg>')
  await render(fence('<svg viewBox="0 0 200 100"><text x="10" y="20">未完成','svg'))
  assert.equal(await page.locator('.svg-preview-image').count(),0)
  assert.equal(await page.locator('.svg-source-pre').isVisible(),true)
  await render('<svg viewBox="0 0 200 100"><text x="10" y="20">未完成')
  assert.equal(await page.locator('.svg-preview-image').count(),0)
  assert.equal(await page.locator('.svg-source-pre').isVisible(),true)
})

test('streaming remains plain text and saved answers render on completion at narrow/dark widths', async () => {
  await render(drawing,true)
  assert.equal(await page.locator('.svg-preview-image').count(),0)
  assert.equal(await page.locator('.md-streaming-plain').textContent(),drawing)
  await page.setViewportSize({width:420,height:900})
  await page.evaluate(()=>document.documentElement.dataset.theme='dark')
  await render(drawing)
  assert.deepEqual((await pixels()).rgba,[37,99,235,255])
  const b=await page.locator('.svg-preview-image').boundingBox()
  assert.ok(b.width<=372)
  assert.ok(Math.abs(b.width/b.height-800/360)<0.01)
  assert.equal(await page.locator('.svg-preview-image').evaluate(img=>getComputedStyle(img).backgroundColor),'rgb(255, 255, 255)')
  if (process.env.ARGUS_TEST_OUTPUT_DIR) await page.locator('#answer').screenshot({path:`${process.env.ARGUS_TEST_OUTPUT_DIR}/svg-narrow-dark.png`})
  await page.setViewportSize({width:1000,height:900})
})
