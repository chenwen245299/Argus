// Run: node --test scripts/test-markdown-math.mjs.
// ARGUS_TEST_PLAYWRIGHT can point to a bundled Playwright index.mjs. Tests the
// real renderer, DOMPurify, KaTeX and copy serializer in an isolated browser.
import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdir } from 'node:fs/promises'
import { createServer } from 'vite'

const { chromium } = await import(process.env.ARGUS_TEST_PLAYWRIGHT || 'playwright')
let server, browser, page, origin
before(async () => {
  server = await createServer({
    configFile: false, root: new URL('../', import.meta.url).pathname,
    appType: 'custom',
    optimizeDeps: { entries: ['src/utils/renderMarkdown.ts', 'src/utils/copyAsMarkdown.ts'] },
    server: { host: '127.0.0.1', port: 0, strictPort: false },
    logLevel: 'error',
  })
  server.middlewares.use((req, res, next) => {
    if (req.url !== '/__math-test') return next()
    res.setHeader('Content-Type', 'text/html; charset=utf-8')
    res.end(`<!doctype html><meta charset="utf-8"><link rel="icon" href="data:,"><style>
      body { margin: 24px; background: #f4f7f8; color: #233140; font: 18px Arial, sans-serif; }
      .markdown-body { max-width: 1100px; line-height: 1.8; }
      .md-inline-code { background: #e2e7ec; padding: 2px 5px; border-radius: 4px; }
      table { border-collapse: collapse; } th, td { border: 1px solid #ccc; padding: 8px 12px; }
      .katex-display { overflow-x: auto; padding: 4px 0; }
    </style><div class="markdown-body" id="answer"></div><script type="module">
      import { renderMarkdown, getSegments } from '/src/utils/renderMarkdown.ts';
      import { fragmentToMarkdown } from '/src/utils/copyAsMarkdown.ts';
      import '/node_modules/katex/dist/katex.min.css';
      window.renderMarkdown = renderMarkdown; window.getSegments = getSegments;
      window.fragmentToMarkdown = fragmentToMarkdown;
    </script>`)
  })
  await server.listen()
  const address = server.httpServer.address()
  origin = `http://127.0.0.1:${address.port}`
  browser = await chromium.launch({ channel: 'chrome', headless: true })
  page = await browser.newPage({ viewport: { width: 1200, height: 800 } })
  page.on('pageerror', error => console.error('Browser error:', error.message))
  page.on('console', message => { if (message.type() === 'error') console.error('Browser console:', message.text()) })
  await page.route('**/*', route => route.request().url().startsWith(origin) ? route.continue() : route.abort())
  await page.goto(`${origin}/__math-test`)
  await page.waitForFunction(() => !!window.renderMarkdown && !!window.fragmentToMarkdown, null, { timeout: 10000 })
})
after(async () => {
  await browser?.close()
  await server?.close()
})

async function render(source) {
  return page.evaluate(source => {
    document.querySelector('#answer').innerHTML = window.renderMarkdown(source)
    const body = document.querySelector('#answer')
    const fragment = document.createDocumentFragment()
    fragment.append(body.cloneNode(true))
    return {
      formulas: body.querySelectorAll('.katex').length,
      displays: body.querySelectorAll('.katex-display').length,
      errors: body.querySelectorAll('.katex-error').length,
      code: [...body.querySelectorAll('.md-inline-code')].map(el => el.textContent),
      blocks: [...body.querySelectorAll('.md-code-block pre code')].map(el => el.textContent),
      sources: [...body.querySelectorAll('annotation[encoding="application/x-tex"]')].map(el => el.textContent),
      copy: window.fragmentToMarkdown(fragment), html: body.innerHTML,
    }
  }, source)
}

const screenshotAnswer = '### 谁在变，谁被固定住\n\n看论文式 (5) 和式 (10) 的变量位置：\n\n'
  + '- **内层**：`q*_{t+1}(·; a,o) ∈ arg min_{q∈Q} F_t(q;a,o)`。变量是 q（信念），a 和 o 是给定的条件。\n'
  + '- **外层**：`min_{a∈A} G_t(a)`。内层算出的 `q*_{t+1}` 在外层作为已知量。\n'

test('the screenshot formulas render as math instead of gray code spans, and remain copyable as TeX', async () => {
  const result = await render(screenshotAnswer)
  assert.equal(result.formulas, 3)
  assert.equal(result.code.length, 0)
  assert.equal(result.errors, 0)
  assert.ok(result.sources[0].includes('q^{*}_{t+1}'))
  assert.ok(result.sources[0].includes('\\operatorname*{arg\\,min}'))
  assert.equal(result.sources[1], '\\min_{a∈A} G_t(a)')
  assert.ok(result.copy.includes('$q^{*}_{t+1}'))
  assert.ok(result.copy.includes('$\\min_{a∈A} G_t(a)$'))
  await page.evaluate(() => document.fonts.ready)
  const painted = await page.locator('.katex-html').evaluateAll(elements => elements.every(el => el.getBoundingClientRect().width > 0 && getComputedStyle(el).fontSize !== '0px'))
  assert.ok(painted)
  if (process.env.ARGUS_TEST_OUTPUT_DIR) {
    await mkdir(process.env.ARGUS_TEST_OUTPUT_DIR, { recursive: true })
    await page.locator('#answer').screenshot({ path: `${process.env.ARGUS_TEST_OUTPUT_DIR}/chat-formulas.png` })
  }
})

test('standard inline and display math still renders with sanitized, copyable source', async () => {
  const result = await render('行内 $x_i$，以及 \\(\\frac{a}{b}\\)。\n\n$$\\sum_{i=1}^{n} x_i$$\n\n\\[ E=mc^2 \\]')
  assert.equal(result.formulas, 4)
  assert.equal(result.displays, 2)
  assert.equal(result.errors, 0)
  assert.deepEqual(result.sources, ['x_i', '\\frac{a}{b}', '\\sum_{i=1}^{n} x_i', 'E=mc^2'])
  assert.ok(result.copy.includes('$$\\sum_{i=1}^{n} x_i$$'))
})

test('math delimiters accidentally wrapped in backticks and clearly mathematical TeX are recovered', async () => {
  const result = await render('`$x_i$`，`\\(q_{t+1}\\)`，`\\frac{a}{b}`，`a_{i} + b_{j}`。')
  assert.equal(result.formulas, 4)
  assert.equal(result.errors, 0)
  assert.equal(result.code.length, 0)
})

test('ordinary code, commands, paths, JSON, prose and invalid TeX remain code', async () => {
  const snippets = ['arxiv_id', 'C_t', 'foo_{bar}', '/tmp/paper_{a}.pdf', 'https://example.org/x_{i}',
    'const x_{i} = 1', 'q_{t+1} => result', '{"x_{i}":1}', 'x_{i};', '这是文字_{不是公式}',
    '\\cite{paper}', '\\frac{a}{', '$\\notacommand{x}$', '$HOME']
  const result = await render(snippets.map(s => '`' + s + '`').join('，'))
  assert.equal(result.formulas, 0)
  assert.deepEqual(result.code, snippets)
  assert.equal(result.errors, 0)
})

test('fenced examples keep their formula source, and longer inline-code fences preserve embedded backticks', async () => {
  const source = '```latex\nq*_{t+1} ∈ Q\n$x_i$\n\\frac{a}{b}\n```\n\n``a `literal` $x_i$``'
  const result = await render(source)
  assert.equal(result.formulas, 0)
  assert.deepEqual(result.blocks, ['q*_{t+1} ∈ Q\n$x_i$\n\\frac{a}{b}'])
  assert.deepEqual(result.code, ['a `literal` $x_i$'])
})

test('formulas inside tables preserve cells and prose surrounding the expressions', async () => {
  const result = await render('| 内层 | 外层 |\n| --- | --- |\n| `q*_{t+1}` | $\\min_a G_t(a)$ |')
  assert.equal(result.formulas, 2)
  assert.equal(result.errors, 0)
  assert.equal((result.html.match(/<td>/g) ?? []).length, 2)
  assert.ok(result.copy.includes('| $q^{*}_{t+1}$ | $\\min_a G_t(a)$ |'))
})

test('math repair retains sanitization and does not execute HTML or trusted TeX commands', async () => {
  const result = await render('<img src=x onerror="window.injected=1">\n\n`$\\href{javascript:alert(1)}{x}$`')
  assert.ok(!result.html.includes('onerror'))
  assert.ok(!result.html.includes('href="javascript:'))
  assert.equal(await page.evaluate(() => window.injected), undefined)
})

test('Mermaid segments and code examples retain their boundaries around math', async () => {
  const segments = await page.evaluate(() => window.getSegments('`q*_{t+1}`\n\n```mermaid\nflowchart LR\nA --> B\n```\n\n$x_i$'))
  assert.deepEqual(segments.map(s => s.type), ['md', 'mermaid', 'md'])
  assert.equal(segments[1].src, 'flowchart LR\nA --> B')
  assert.ok(segments[0].html.includes('class="katex"'))
  assert.ok(segments[2].html.includes('class="katex"'))
})
