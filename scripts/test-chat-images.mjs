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
