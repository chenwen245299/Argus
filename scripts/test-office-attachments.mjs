import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'
const transpile = source => ts.transpileModule(source, { compilerOptions: { target:ts.ScriptTarget.ES2021,module:ts.ModuleKind.CommonJS } }).outputText
const api={}
new Function('exports',transpile(await readFile(new URL('../src/utils/attachments.ts',import.meta.url),'utf8')))(api)
const fixtures=[['report.doc','application/msword','word','doc'],['report.docx','application/vnd.openxmlformats-officedocument.wordprocessingml.document','word','docx'],
  ['slides.ppt','application/vnd.ms-powerpoint','powerpoint','ppt'],['slides.pptx','application/vnd.openxmlformats-officedocument.presentationml.presentation','powerpoint','pptx'],
  ['values.xls','application/vnd.ms-excel','excel','xls'],['values.xlsx','application/vnd.openxmlformats-officedocument.spreadsheetml.sheet','excel','xlsx'],
  ['values.csv','text/csv','excel','csv'],['values.tsv','text/tab-separated-values','excel','tsv']]
test('all new and legacy Office formats work by MIME and by extension, including Excel-exported CSV',()=>{
  for(const[name,type,kind,format]of fixtures){
    assert.equal(api.attachmentTypeFor({name,type}),kind)
    assert.equal(api.attachmentTypeFor({name:name.toUpperCase(),type:''}),kind)
    assert.equal(api.officeFormatFor({name,type}),format)
    assert(api.ATTACHMENT_ACCEPT.includes('.'+format))
  }
  assert.equal(api.officeFormatFor({name:'values.csv',type:'application/vnd.ms-excel'}),'csv')
  assert.equal(api.attachmentTypeFor({name:'image.png',type:'image/png'}),'image')
  assert.equal(api.attachmentTypeFor({name:'file.exe',type:'application/x-msdownload'}),null)
})
test('Office original bytes are internal IPC parts only, with exact preserved format/id/name',()=>{
  for(const[name,mime,type,format]of fixtures){
    const attachment={id:format,type,name,format,dataUrl:`data:${mime};base64,local-fixture`}
    const parts=api.buildContentParts('Read this',[attachment])
    assert.deepEqual(parts[1],{type:'local_document',id:format,filename:name,format,data_url:attachment.dataUrl})
    assert(!parts.some(p=>p.type==='file'||p.type==='input_file'))
  }
})
test('size limits are checked before reading Office bytes, with kind-specific UI metadata',async()=>{
  for(const[name,type,kind]of fixtures){
    const result=await api.readAttachmentFile({name,type,size:api.MAX_LOCAL_PDF_BYTES+1})
    assert.deepEqual(result,{status:'too-large',name,type:kind,limitMb:64})
  }
})
test('saved and queued attachment snapshots retain new and old format metadata',()=>{
  const atts=fixtures.map(([name,mime,type,format])=>({id:format,type,name,format,dataUrl:`data:${mime};base64,fixture`}))
  const saved=JSON.parse(JSON.stringify(atts))
  assert.deepEqual(api.buildContentParts('Follow up',saved),api.buildContentParts('Follow up',atts))
})
test('Office draft context counts descriptors rather than bytes or vision tokens',async()=>{
  const context={}
  new Function('exports',transpile(await readFile(new URL('../src/utils/chatContext.ts',import.meta.url),'utf8')))(context)
  for(const[name,,type]of fixtures){
    const usage=context.withContextDraft({usedTokens:100,contextWindow:10000,messageTokens:50,mediaEstimated:false},'',
      [{name,type,get dataUrl(){throw Error('Document bytes must not be scanned')}}])
    assert(usage.usedTokens<300);assert.equal(usage.mediaEstimated,false)
  }
})
