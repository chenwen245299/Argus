// Real attachment/history helpers; local fixtures only, no provider/library IO.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const transpile = source => ts.transpileModule(source, { compilerOptions: {
  target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.CommonJS,
} }).outputText
const api = {}
new Function('exports', transpile(await readFile(new URL('../src/utils/attachments.ts', import.meta.url), 'utf8')))(api)
async function historyFunction(file, name, prelude) {
  const source = await readFile(new URL('../' + file, import.meta.url), 'utf8')
  const script = source.slice(source.indexOf('>') + 1, source.indexOf('</script>'))
  const ast = ts.createSourceFile(file, script, ts.ScriptTarget.Latest, true)
  const node = ast.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name)
  return new Function('buildContentParts', transpile(`${prelude}\n${node.getText(ast)}\nreturn ${name}`))(api.buildContentParts)
}
const paperHistory = await historyFunction('src/components/tabs/AiTab.vue', 'buildHistoryUntil',
  'const activeAnswerForGroup=g=>g.answers[0]; const buildToolExchangeMessages=()=>[];')
const libraryHistory = await historyFunction('src/components/LibraryChat.vue', 'chatHistoryFromMessages',
  'const activeAnswer=m=>m; const buildToolExchangeMessages=()=>[];')
const pdf = { id: 'attachment-1', type: 'pdf', name: 'supplement.pdf', dataUrl: 'data:application/pdf;base64,JVBERi0=' }

test('PDFs use internal local-tool parts alongside unchanged image/audio/video attachments', () => {
  const result = api.buildContentParts('Read this', [pdf,
    {type:'image',dataUrl:'data:image/png;base64,AA',detail:'low'},
    {type:'audio',dataUrl:'data:audio/wav;base64,AA'},
    {type:'video',dataUrl:'data:video/mp4;base64,AA'}])
  assert.deepEqual(result[1], {type:'local_pdf',id:pdf.id,filename:pdf.name,data_url:pdf.dataUrl})
  assert.deepEqual(result.map(p=>p.type), ['text','local_pdf','image_url','input_audio','video_url'])
  assert.equal(result[2].image_url.detail, 'low')
  assert(!result.some(p=>p.type==='file' || p.type==='input_file'))
})

test('paper and library follow-ups/regeneration/reload keep PDFs on the local-tool path', () => {
  const user = {id:'user',role:'user',content:'Can you read this?',attachments:[pdf]}
  const reply = {id:'answer',role:'assistant',content:'Yes'}
  const paper = {id:'paper-chat',nodes:[user,{id:'group',role:'assistantGroup',answers:[reply]},
    {id:'next',role:'user',content:'Read page 2'}]}
  const library = [user,reply,{id:'next',role:'user',content:'Read page 2'}]
  for (const messages of [paperHistory(paper),paperHistory(JSON.parse(JSON.stringify(paper))),
    paperHistory(paper,'group'),libraryHistory(library),libraryHistory(JSON.parse(JSON.stringify(library)))]) {
    assert.equal(messages[0].content[1].type,'local_pdf')
    assert.equal(messages[0].content[1].data_url,pdf.dataUrl)
    assert(!messages.flatMap(m=>Array.isArray(m.content)?m.content:[]).some(p=>p.type==='file'))
  }
})

test('PDF files with missing MIME type can be attached; oversized ones stop before reading', async () => {
  assert.equal(api.attachmentTypeFor({name:'Supplement.PDF',type:''}), 'pdf')
  assert.equal(api.attachmentTypeFor({name:'document.txt',type:''}), null)
  assert.deepEqual(await api.readAttachmentFile({name:'large.pdf',type:'application/pdf',size:api.MAX_LOCAL_PDF_BYTES+1}),
    {status:'too-large',name:'large.pdf',type:'pdf',limitMb:64})
})

test('PDF drafts estimate only the local descriptor and never inspect base64', async () => {
  const context = {}
  new Function('exports', transpile(await readFile(new URL('../src/utils/chatContext.ts', import.meta.url), 'utf8')))(context)
  const projected = context.withContextDraft({usedTokens:100,contextWindow:10000,messageTokens:40,mediaEstimated:false},'',
    [{...pdf,get dataUrl(){throw Error('must not scan PDF')}}])
  assert(projected.usedTokens < 300)
  assert.equal(projected.mediaEstimated,false)
})
