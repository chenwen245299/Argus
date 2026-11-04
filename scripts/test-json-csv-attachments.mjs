import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const transpile = source => ts.transpileModule(source, { compilerOptions: { target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.CommonJS } }).outputText
const api={}
new Function('exports',transpile(await readFile(new URL('../src/utils/attachments.ts',import.meta.url),'utf8')))(api)
test('JSON and CSV are detected through extensions and common MIME variants',()=>{
  for(const [name,type,expected] of [['data.JSON','','json'],['upload','application/json','json'],['upload','text/json','json'],['upload','application/ld+json','json'],
    ['data.csv','application/vnd.ms-excel','excel'],['upload','application/csv','excel'],['upload','text/comma-separated-values','excel']]) {
    assert.equal(api.attachmentTypeFor({name,type}),expected)
  }
  assert(api.ATTACHMENT_ACCEPT.includes('.json')&&api.ATTACHMENT_ACCEPT.includes('.csv'))
  assert.equal(api.isLocalDocumentAttachment({type:'json'}),true)
})
test('JSON and CSV are local references throughout save/reload, without provider file blocks',()=>{
  const attachments=[{id:'json',type:'json',name:'data.json',dataUrl:'data:application/json;base64,original-json'},
    {id:'csv',type:'excel',format:'csv',name:'table.csv',dataUrl:'data:text/csv;base64,original-csv'}]
  for(const files of [attachments,JSON.parse(JSON.stringify(attachments))]){
    const parts=api.buildContentParts('Read the data',files)
    assert.deepEqual(parts.slice(1).map(p=>[p.type,p.format]),[['local_document','json'],['local_document','csv']])
    assert.equal(parts[1].data_url,attachments[0].dataUrl)
    assert(!parts.some(p=>p.type==='file'||p.type==='input_file'))
  }
})
test('JSON and CSV are capped before FileReader and count only descriptors in draft context',async()=>{
  const context={}
  new Function('exports',transpile(await readFile(new URL('../src/utils/chatContext.ts',import.meta.url),'utf8')))(context)
  for(const [name,mime,type] of [['data.json','application/json','json'],['table.csv','text/csv','excel']]){
    assert.equal((await api.readAttachmentFile({name,type:mime,size:api.MAX_LOCAL_PDF_BYTES+1})).status,'too-large')
    const projected=context.withContextDraft({usedTokens:100,contextWindow:10000,messageTokens:50,mediaEstimated:false},'',
      [{type,name,get dataUrl(){throw Error('Must not scan original data')}}])
    assert(projected.usedTokens<300);assert.equal(projected.mediaEstimated,false)
  }
})
