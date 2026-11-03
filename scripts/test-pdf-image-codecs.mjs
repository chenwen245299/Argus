// Real PDF.js worker + the actual PdfViewer getDocument options. Localhost only.
// Optional: ARGUS_TEST_PDF=/absolute/path/to/a/scanned.pdf validates a real file
// without copying it or writing to its library.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createServer } from 'vite'
import { parse } from '@vue/compiler-sfc'
import ts from 'typescript'
const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
const source = await readFile(new URL('../src/components/PdfViewer.vue', import.meta.url), 'utf8')
const script = parse(source).descriptor.scriptSetup.content
const ast = ts.createSourceFile('PdfViewer.ts', script, ts.ScriptTarget.Latest, true)
let options
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(ast) === 'pdfjsLib.getDocument') options = node.arguments[0].getText(ast)
  ts.forEachChild(node, visit)
}
visit(ast)
assert.ok(options)
const optionsFunction = ts.transpileModule(`function viewerOptions(uint8) { const PDF_ENABLE_HWA = true; return (${options}); }`, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
// A lossless OpenJPEG-encoded 32x32 pattern we generated: red/green/blue/black
// quadrants, with no external document content or fonts.
const jpx = Buffer.from('AAAADGpQICANCocKAAAAFGZ0eXBqcDIgAAAAAGpwMiAAAAAtanAyaAAAABZpaGRyAAAAIAAAACAAAwcHAAAAAAAPY29scgEAAAAAABAAAAJvanAyY/9P/1EALwAAAAAAIAAAACAAAAAAAAAAAAAAACAAAAAgAAAAAAAAAAAAAwcBAQcBAQcBAf9SAAwAAAABAAUEBAAB/1wAE0BASEhQSEhQSEhQSEhQSEhQ/2QAJQABQ3JlYXRlZCBieSBPcGVuSlBFRyB2ZXJzaW9uIDIuNS40/5AACgAAAAAB6AAB/5PH1AQEc8+0CAW/z7QIBY/PwAp+AFH4AQAFrwWvAh/PwAofUBR+AEADGQXfBh/D6gKfgBR+AEAF3wMZBh/PwBp+ANB9QKAE8IDjE78ExTR/a38AAQRSv8/AHj7QWD6gUAegiOOO/38NkskKrQlonimPx9oLPwBoPqBQC31mi2kHdT4cNpMJaJ4pv8/ALn4BUPtBwBe+AMtoj5nr0JoPDxbPQsIbklfM7Rav3y0ah2fPwC5+ATD7QcAXEf7LaI+Z89jNlyDcXnTVtIcZTxYD3S0acc3PwCZ+AXD7QcAZ5QfYG/JMEB8MZsdCwhuSXxhuaxYD3S0acc/PwDZ+AZD7QgAiGQSERSWlGYYmzw1/A3raWYPPdXg8oxpGJCwrnA/cNX/PwD5+AZD7QgAhbQKERSYxDMWl85AqCL8uQjHnbZ34KsZM1xMk3VBZcj+yxc/AMn4B0PtCAEnN2y7G7YKAYRoHnwDK0lmDz5hODyX8Rc6OJN1QWXI/stPPwCZ+ATD7QQA2njmcgr221T9mrfKU3ijR2m+DFPh5z8AyfgEQ+0EAgv4OwmKOvAgNUmsXhTCoZFrqS1yC/zwcz8AmfgGwfUCAk4/+zVRED86Xgv3voEJeuefD2wfSj4L/RF//2Q==', 'base64')
function patternPdf() {
  const content = Buffer.from('q 200 0 0 200 0 0 cm /Pattern Do Q')
  const objects = [
    Buffer.from('<< /Type /Catalog /Pages 2 0 R >>'),
    Buffer.from('<< /Type /Pages /Count 1 /Kids [3 0 R] >>'),
    Buffer.from('<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /XObject << /Pattern 5 0 R >> >> /Contents 4 0 R >>'),
    Buffer.concat([Buffer.from(`<< /Length ${content.length} >>\nstream\n`), content, Buffer.from('\nendstream')]),
    Buffer.concat([Buffer.from(`<< /Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /JPXDecode /Length ${jpx.length} >>\nstream\n`),jpx,Buffer.from('\nendstream')]),
  ]
  const chunks=[Buffer.from('%PDF-1.5\n')], offsets=[0]
  for(let i=0;i<objects.length;i++){offsets.push(Buffer.concat(chunks).length);chunks.push(Buffer.from(`${i+1} 0 obj\n`),objects[i],Buffer.from('\nendobj\n'))}
  const start=Buffer.concat(chunks).length
  chunks.push(Buffer.from(`xref\n0 6\n0000000000 65535 f \n${offsets.slice(1).map(n=>String(n).padStart(10,'0')+' 00000 n \n').join('')}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${start}\n%%EOF\n`))
  return Buffer.concat(chunks)
}
let server,browser,page,origin
const warnings=[], requests=[], errors=[]
before(async()=>{
  server=await createServer({configFile:false,root:new URL('../',import.meta.url).pathname,appType:'custom',
    cacheDir:'node_modules/.vite-pdf-codecs-test',optimizeDeps:{include:['pdfjs-dist/legacy/build/pdf.mjs']},
    server:{host:'127.0.0.1',port:0},logLevel:'error'})
  server.middlewares.use(async(req,res,next)=>{
    if(req.url==='/__pattern.pdf'){res.setHeader('Content-Type','application/pdf');res.end(patternPdf());return}
    if(req.url==='/__original.pdf'&&process.env.ARGUS_TEST_PDF){res.setHeader('Content-Type','application/pdf');res.end(await readFile(process.env.ARGUS_TEST_PDF));return}
    if(!req.url.startsWith('/__pdf-codecs'))return next()
    res.setHeader('Content-Type','text/html; charset=utf-8')
    res.end(await server.transformIndexHtml(req.url,`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><canvas></canvas>
      <script type="module">
      import*as pdfjsLib from'pdfjs-dist/legacy/build/pdf.mjs';
      import worker from'pdfjs-dist/legacy/build/pdf.worker.min.mjs?url';pdfjsLib.GlobalWorkerOptions.workerSrc=worker;
      ${optionsFunction}
      window.drawPdf=async({original=false,number=1,missing=false,jsOnly=false}={})=>{
        const data=new Uint8Array(await(await fetch(original?'/__original.pdf':'/__pattern.pdf')).arrayBuffer());
        const options=viewerOptions(data);if(missing){delete options.wasmUrl;delete options.iccUrl}if(jsOnly)options.useWasm=false;
        const doc=await pdfjsLib.getDocument(options).promise;
        const page=await doc.getPage(number), initial=page.getViewport({scale:1});
        const viewport=page.getViewport({scale:600/Math.max(initial.width,initial.height)});
        const canvas=document.querySelector('canvas');canvas.width=Math.ceil(viewport.width);canvas.height=Math.ceil(viewport.height);
        await page.render({canvas,viewport}).promise;
        const ctx=canvas.getContext('2d'),pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data;
        let nonwhite=0,total=0;for(let i=0;i<pixels.length;i+=64){total++;if(pixels[i]<240||pixels[i+1]<240||pixels[i+2]<240)nonwhite++}
        const sample=(x,y)=>Array.from(ctx.getImageData(Math.floor(x*canvas.width),Math.floor(y*canvas.height),1,1).data);
        const result={pages:doc.numPages,nonwhite:nonwhite/total,colors:[sample(.25,.25),sample(.75,.25),sample(.25,.75),sample(.75,.75)]};
        await doc.destroy();return result;
      };window.ready=true;
      </script>`))
  })
  await server.listen();origin='http://127.0.0.1:'+server.httpServer.address().port
  browser=await chromium.launch({channel:'chrome',headless:true});page=await browser.newPage()
  page.on('console',message=>{if(['warning','error'].includes(message.type()))warnings.push(message.text())})
  page.on('pageerror',error=>errors.push(error.message))
  page.on('request',request=>requests.push(request.url()))
  await page.route('**/*',route=>route.request().url().startsWith(origin)?route.continue():route.abort())
})
after(async()=>{await browser?.close();await server?.close()})
async function draw(options={}){await page.goto(origin+'/__pdf-codecs');await page.waitForFunction(()=>window.ready);return page.evaluate(options=>window.drawPdf(options),options)}

test('all decoder binaries, script fallbacks and color profiles match the installed PDF.js version',async()=>{
  for(const asset of ['wasm/openjpeg.wasm','wasm/jbig2.wasm','wasm/qcms_bg.wasm',
    'wasm/openjpeg_nowasm_fallback.js','wasm/jbig2_nowasm_fallback.js','iccs/CGATS001Compat-v2-micro.icc']){
    const [copied,installed]=await Promise.all([
      readFile(new URL('../public/pdfjs/'+asset,import.meta.url)),
      readFile(new URL('../node_modules/pdfjs-dist/'+asset,import.meta.url)),
    ])
    assert.deepEqual(copied,installed,asset)
  }
})
test('missing image decoder configuration reproduces a silent white page',async()=>{
  const result=await draw({missing:true})
  assert.equal(result.pages,1);assert.equal(result.nonwhite,0)
  assert.ok(warnings.some(message=>/OpenJPEG|wasmUrl|JpxImage/.test(message)))
})
test('the real viewer configuration decodes JPEG 2000 images to their expected colors',async()=>{
  const result=await draw()
  assert.ok(result.nonwhite>.99,JSON.stringify({result,warnings}))
  assert.deepEqual(result.colors.map(color=>color.slice(0,3)),[[255,0,0],[0,255,0],[0,0,255],[0,0,0]])
  assert.ok(requests.some(url=>url.endsWith('/pdfjs/wasm/openjpeg.wasm')))
})
test('the packaged JavaScript decoder works when WebAssembly is unavailable',async()=>{
  const result=await draw({jsOnly:true})
  assert.ok(result.nonwhite>.99)
  assert.ok(requests.some(url=>url.endsWith('/pdfjs/wasm/openjpeg_nowasm_fallback.js')))
  assert.deepEqual(errors,[])
})
test('the original scanned book renders its cover and body pages', {skip:!process.env.ARGUS_TEST_PDF},async()=>{
  const before=await draw({original:true,missing:true})
  assert.ok(before.nonwhite<.001,'The original file must reproduce the pre-fix white page')
  const start=warnings.length
  for(const number of [1,3,20]){
    const result=await draw({original:true,number})
    assert.ok(result.nonwhite>.01,`Page ${number} was blank: ${JSON.stringify(result)}`)
    if(process.env.ARGUS_TEST_OUTPUT_DIR)await page.locator('canvas').screenshot({path:process.env.ARGUS_TEST_OUTPUT_DIR+'/book-page-'+number+'.png'})
  }
  assert.ok(requests.some(url=>url.endsWith('/pdfjs/wasm/jbig2.wasm')))
  assert.ok(!warnings.slice(start).some(message=>/Unable to decode image|failed to initialize/.test(message)))
})
