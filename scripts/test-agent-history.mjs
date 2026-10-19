// Real history/persistence helpers with local fixtures; no library or model I/O.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const transpile = source => ts.transpileModule(source, { compilerOptions: {
  target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext,
} }).outputText
const { buildToolExchangeMessages } = await import('data:text/javascript;base64,' + Buffer.from(transpile(
  await readFile(new URL('../src/utils/agentHistory.ts', import.meta.url), 'utf8'),
)).toString('base64'))
async function realFunctions(file, names, prelude = '') {
  const source = await readFile(new URL('../' + file, import.meta.url), 'utf8')
  const script = source.slice(source.indexOf('>') + 1, source.indexOf('</script>'))
  const ast = ts.createSourceFile(file, script, ts.ScriptTarget.Latest, true)
  const code = names.map(name => {
    const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name)
    assert.ok(node, name); return node.getText(ast)
  }).join('\n')
  return new Function('buildToolExchangeMessages', transpile(`${prelude}\n${code}\nreturn {${names.join(',')}}`))(buildToolExchangeMessages)
}
const paper = await realFunctions('src/components/tabs/AiTab.vue', [
  'cloneConversation', 'persistableSteps', 'persistableConversation', 'activeAnswerForGroup', 'buildHistoryUntil',
], 'const activeAnswerTabs={value:{}}; const persistableServerToolTrace=x=>x;')
const library = await realFunctions('src/components/LibraryChat.vue', ['persistableSteps', 'chatHistoryFromMessages'], 'const activeAnswer=x=>x;')
const canvas = await realFunctions('src/components/tabs/CanvasChatTab.vue', ['rawContextHistory'])
const image = { slug: 'paper', page: 15, file: 'figure.png', crop: {x:.1,y:.2,width:.3,height:.4}, dataUrl:'data:image/png;base64,live-only' }
const step = preview => ({ tool:'get_paper_fulltext', args:'', argsJson:'{"slug":"paper"}', ok:true, preview, chars:preview.length })

test('all 23 questions and answers plus the entire first-read paper survive save/reload and follow-up', () => {
  const text = 'paper-evidence-0123456789 '.repeat(3600)
  const slices = [text.slice(0,20000),text.slice(20000,45000),text.slice(45000,75000),text.slice(75000)]
  const conv = {id:'conversation',slug:'paper',nodes:[]}
  for(let i=1;i<=23;i++){
    conv.nodes.push({id:'u'+i,role:'user',content:'Question '+i})
    conv.nodes.push({id:'g'+i,role:'assistantGroup',answers:[{id:'a'+i,content:'Answer '+i,
      steps:i===1?[...slices.map(step),{tool:'view_paper_page',args:'',argsJson:'{"pages":[15]}',ok:true,preview:'Figure text',images:[image]}]:undefined}]})
  }
  const outgoing = paper.buildHistoryUntil(conv)
  const restored = JSON.parse(JSON.stringify(paper.persistableConversation(conv)))
  const after = paper.buildHistoryUntil(restored)
  assert.deepEqual(after,outgoing,'reload must send identical text and image references')
  for(let i=1;i<=23;i++){
    assert(after.some(m=>m.role==='user'&&m.content==='Question '+i))
    assert(after.some(m=>m.role==='assistant'&&m.content==='Answer '+i))
  }
  assert.equal(after.filter(m=>m.role==='tool'&&m.content!=='Figure text').map(m=>m.content).join(''),text)
  const pixels=after.flatMap(m=>Array.isArray(m.content)?m.content:[]).filter(p=>p.type==='image_url')
  assert.equal(pixels.length,1)
  const ref=JSON.parse(Buffer.from(pixels[0].image_url.url.replace('argus-chat-image:',''),'base64url').toString())
  assert.deepEqual(ref,{conversationId:conv.id,file:image.file})
  assert(!JSON.stringify(restored).includes('live-only'))
  const calls=after.find(m=>m.tool_calls).tool_calls
  assert.deepEqual(calls.map(c=>c.id),after.filter(m=>m.role==='tool').map(m=>m.tool_call_id))
})

test('paper and library persist every tool result beyond the former per-step and per-answer caps', () => {
  const steps=Array.from({length:8},(_,i)=>step(('source-'+i+' ').repeat(30000)))
  for(const helper of [paper,library]){
    const saved=JSON.parse(JSON.stringify(helper.persistableSteps(steps)))
    assert.deepEqual(saved,steps)
    assert.deepEqual(buildToolExchangeMessages(saved,'answer','conversation').filter(m=>m.role==='tool').map(m=>m.content),steps.map(s=>s.preview))
  }
})

test('library and canvas replay full tool evidence and labelled saved images', () => {
  const steps=[step('Exact paper result'),{tool:'view_paper_page',ok:true,preview:'Figure text',images:[image]}]
  const records=[{id:'u',role:'user',content:'Question'},{id:'a',role:'assistant',content:'Answer',steps,agentSteps:steps}]
  for(const messages of [library.chatHistoryFromMessages(records,'library-conversation'),canvas.rawContextHistory(records,'canvas-conversation')]){
    assert(messages.some(m=>m.role==='tool'&&m.content==='Exact paper result'))
    assert(messages.some(m=>Array.isArray(m.content)&&m.content.some(p=>p.type==='image_url'&&p.image_url.url.startsWith('argus-chat-image:'))))
    assert.equal(messages.at(-1).content,'Answer')
  }
})

test('legacy clipped results are explicit and interrupted or failed image calls cannot invent pixels', () => {
  const messages=buildToolExchangeMessages([
    {...step('retained prefix'),chars:100000},
    {tool:'unfinished',images:[image]},
    {tool:'failed',ok:false,images:[image]},
  ],'answer','conversation')
  assert.match(messages.find(m=>m.role==='tool').content,/legacy saved result is incomplete/)
  assert.equal(messages.filter(m=>m.role==='tool').length,2)
  assert(!messages.some(m=>Array.isArray(m.content)))
})
