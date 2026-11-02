// Run: node --test scripts/test-chat-images.mjs. No desktop or provider needed.
import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import ts from 'typescript'

const dir = await mkdtemp(join(tmpdir(), 'argus-chat-images-test-'))
after(() => rm(dir, { recursive: true, force: true }))
for (const [source, name] of [['../src/types/chatImages.ts', 'types'], ['../src/utils/chatImageContent.ts', 'content']]) {
  const text = await readFile(new URL(source, import.meta.url), 'utf8')
  const js = ts.transpileModule(text, { compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext } }).outputText
  await writeFile(join(dir, `${name}.mjs`), js.replace(/from '\.\.\/types\/chatImages'/g, "from './types.mjs'"))
}
const { chatImageContent } = await import(pathToFileURL(join(dir, 'content.mjs')))
const image = file => ({ slug: 'paper', page: 3, file, caption: 'Figure 3' })
const a = image('image-a.png'), b = image('image-b.png')
const steps = [{ tool: 'send_paper_image', ok: true, images: [a, b] }]

test('images follow the paragraph references, independent of send order', () => {
  const content = '## Figure B\n\n![](argus-image://image-b.png)\n\n解释 B。\n\n## Figure A\n\n![](argus-image://image-a.png)\n\n解释 A。'
  const body = chatImageContent(content, steps)
  assert.deepEqual(body.parts.map(part => part.type), ['text', 'image', 'text', 'image', 'text'])
  assert.equal(body.parts[1].image, b)
  assert.equal(body.parts[3].image, a)
  assert.match(body.parts[2].content, /解释 B/)
  assert.deepEqual(body.remaining, [])
})

test('saved conversations reload the same inline positions without live image blobs', () => {
  const persisted = JSON.parse(JSON.stringify(steps))
  const body = chatImageContent('正文\n\n![](argus-image://image-a.png)\n\n后文', persisted)
  assert.equal(body.parts[1].image.file, a.file)
  assert.equal(body.parts[1].image.dataUrl, undefined)
  assert.deepEqual(body.remaining, [b])
})

test('legacy and interrupted answers retain every unreferenced sent image', () => {
  assert.deepEqual(chatImageContent('旧回答', steps).remaining, [a, b])
  const partial = chatImageContent('正文\n![](argus-image://image-a.png)\n未完', steps)
  assert.deepEqual(partial.remaining, [b])
  assert.equal(chatImageContent('', [{ tool: 'view_paper_page', ok: true, images: [a] }]).remaining.length, 0)
})

test('code examples, inline code and unknown or failed tool references do not load files', () => {
  for (const content of [
    '```markdown\n![](argus-image://image-a.png)\n```',
    '~~~markdown\n![](argus-image://image-a.png)\n~~~',
    '`![](argus-image://image-a.png)`',
    '![](argus-image://../../secret.png)',
    '![](argus-image://unknown.png)',
  ]) {
    const body = chatImageContent(content, steps)
    assert.deepEqual(body.parts, [{ type: 'text', content }])
    assert.deepEqual(body.remaining, [a, b])
  }
  assert.equal(chatImageContent('![](argus-image://image-a.png)', [{ ...steps[0], ok: false }]).parts[0].type, 'text')
})

test('Markdown maths, lists and Mermaid blocks remain untouched around images', () => {
  const before = '1. 先看公式\n2. 再看曲线\n\n$$E(x)=x^2$$\n\n'
  const after = '\n```mermaid\ngraph LR\nA-->B\n```\n'
  const body = chatImageContent(`${before}![](argus-image://image-a.png)\n${after}`, steps)
  assert.equal(body.parts[0].content, before)
  assert.equal(body.parts[2].content, after)
})

const c = image('image-c.png'), d = image('image-d.png')
const gridSteps = [{ tool: 'send_paper_image', ok: true, images: [d, b, c, a] }]
const ref = image => `![](argus-image://${image.file})`
function table(rows) {
  const row = cells => '| ' + cells.map(value => value ? ref(value) : '').join(' | ') + ' |'
  return [row(rows[0]), '| ' + rows[0].map(() => '---').join(' | ') + ' |', ...rows.slice(1).map(row)].join('\n')
}

test('1x2, 2x2 and 2x1 grids retain requested positions, independent of send order', () => {
  for (const rows of [[[a,b]], [[a,b],[c,d]], [[a],[b]], [[a,b,c,d]]]) {
    const content = '说明 $x^2$。\n\n' + table(rows) + '\n\n继续讲解。'
    const body = chatImageContent(content, gridSteps)
    assert.deepEqual(body.parts.map(part => part.type), ['text','imageGrid','text'])
    assert.equal(body.parts[1].columns, rows[0].length)
    assert.equal(body.parts[1].rows, rows.length)
    assert.deepEqual(body.parts[1].cells, rows.flat())
    assert.equal(body.parts[0].content, '说明 $x^2$。\n\n')
    assert.equal(body.parts[2].content, '\n继续讲解。')
    assert.deepEqual(body.remaining, gridSteps[0].images.filter(image => !rows.flat().includes(image)))
    assert.deepEqual(chatImageContent(content, JSON.parse(JSON.stringify(gridSteps))), body, 'reload retains layout')
  }
})

test('blank cells retain their positions; multiple grids and single figures can be interleaved', () => {
  const content = table([[a,null],[b,c]]) + '\n\n细节\n\n' + ref(d)
  const body = chatImageContent(content, gridSteps)
  assert.deepEqual(body.parts[0], { type: 'imageGrid', columns: 2, rows: 2, cells: [a,null,b,c] })
  assert.deepEqual(body.parts.at(-1), { type: 'image', image: d })
  assert.deepEqual(body.remaining, [])
  const twice = chatImageContent(table([[a,b]]) + '\n\n比较。\n\n' + table([[c,d]]), gridSteps)
  assert.deepEqual(twice.parts.map(p => p.type), ['imageGrid','text','imageGrid'])
})

test('mixed, unknown, failed, malformed and code-fenced tables never authorize image reads', () => {
  for (const source of [
    `| 说明 | 数值 |\n| --- | --- |\n| ${ref(a)} | 42 |`,
    table([[a,{file:'unknown.png'}]]),
    table([[a,{file:'../../secret.png'}]]),
    table([[a,b]]) + '\n| ' + ref(c) + ' |',
    table([[a,b]]) + '\n| ' + ref(c),
    '```markdown\n' + table([[a,b]]) + '\n```',
    '~~~\n' + table([[a,b]]) + '\n~~~',
    '    ' + table([[a,b]]).replaceAll('\n','\n    '),
    table([[a,b,c,d,a]]),
    table([[a,b,c],[a,b,c],[a,b,c]]),
  ]) {
    assert.deepEqual(chatImageContent(source, gridSteps).parts, [{ type: 'text', content: source }])
  }
  assert.deepEqual(chatImageContent(table([[a,b]]), [{ ...gridSteps[0], ok: false }]).parts,
    [{ type: 'text', content: table([[a,b]]) }])
})

test('streaming tables keep unsent or partial references as text until the row is complete', () => {
  const source = table([[a,b],[c,d]])
  assert.equal(chatImageContent(source.slice(0,-5), gridSteps).parts[0].type, 'text')
  const completed = chatImageContent(source, gridSteps)
  assert.equal(completed.parts[0].type, 'imageGrid')
  assert.deepEqual(completed.remaining, [])
})
