// Actual PaperList selection/action functions; never touches the system clipboard.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { ref, computed } from 'vue'
import { parse } from '@vue/compiler-sfc'
import ts from 'typescript'
const source = await readFile(new URL('../src/components/PaperList.vue', import.meta.url), 'utf8')
const sfc = parse(source).descriptor
const ast = ts.createSourceFile('PaperList.ts', sfc.scriptSetup.content, ts.ScriptTarget.Latest, true)
const names = ['ctxTargets', 'ctxIsMulti', 'ctxPdfCount', 'closeCtx', 'copyPaperPdf', 'copyPaperPdfs']
const statements = ast.statements.filter(n => ts.isFunctionDeclaration(n) ? names.includes(n.name?.text) : ts.isVariableStatement(n) && n.declarationList.declarations.some(d => names.includes(d.name.getText(ast))))
assert.equal(statements.length, names.length)
const code = ts.transpileModule(statements.map(n => n.getText(ast)).join('\n'), {compilerOptions:{target:ts.ScriptTarget.ES2021}}).outputText
const paper = (slug, file_type='pdf') => ({slug, file_type})
function fixture(papers, selected=[]) {
  const ctxMenu=ref({item:papers[0]}), selection={selectedSlugs:selected}, library={currentPath:'/library-a'}
  const calls=[], notices=[], errors=[]
  let reject, finish
  const invoke=(name,args)=>{calls.push({name,args});return new Promise((resolve,r)=>{finish=resolve;reject=r})}
  const api=new Function('computed','ctxMenu','selection','sorted','library','invoke','isEbookFileType','showNotice','showError','t','blankCtx',code+'\nreturn {ctxTargets,ctxIsMulti,ctxPdfCount,copyPaperPdf,copyPaperPdfs}')(computed,ctxMenu,selection,ref(papers),library,invoke,type=>['epub','mobi','azw3','txt','md'].includes(type),text=>notices.push(text),text=>errors.push(text),(key,args)=>`${key}:${args.n}`,ref(null))
  return {...api,ctxMenu,library,calls,notices,errors,finish:n=>finish(n),fail:e=>reject(e)}
}
test('eight selected PDFs are captured in one native file-copy call before closing the menu',async()=>{
  const papers=Array.from({length:8},(_,i)=>paper('paper-'+i)),h=fixture(papers,papers.map(p=>p.slug))
  assert.equal(h.ctxIsMulti.value,true);assert.equal(h.ctxPdfCount.value,8)
  const copy=h.copyPaperPdfs(h.ctxTargets.value)
  assert.equal(h.ctxMenu.value,null);assert.equal(h.calls.length,1)
  assert.deepEqual(h.calls[0],{name:'copy_paper_pdfs',args:{slugs:papers.map(p=>p.slug),expectedRoot:'/library-a'}})
  h.finish(8);await copy;assert.deepEqual(h.notices,['paper.pdfCopied:8'])
})
test('mixed selection copies PDFs only, with legacy PDF types and stable displayed order',async()=>{
  const papers=[paper('old',undefined),paper('ebook','epub'),paper('pdf'),paper('text','txt')]
  papers[0].file_type=undefined
  const h=fixture(papers,['pdf','ebook','old','text'])
  assert.equal(h.ctxPdfCount.value,2)
  const copy=h.copyPaperPdfs(h.ctxTargets.value);assert.deepEqual(h.calls[0].args.slugs,['old','pdf'])
  h.finish(2);await copy
})
test('single selection uses the same native operation and non-PDF selections do not copy',async()=>{
  const h=fixture([paper('one')]);const copy=h.copyPaperPdf(paper('one'))
  assert.deepEqual(h.calls[0].args.slugs,['one']);h.finish(1);await copy
  const ebooks=fixture([paper('a','epub'),paper('b','mobi')],['a','b'])
  await ebooks.copyPaperPdfs(ebooks.ctxTargets.value)
  assert.equal(ebooks.ctxPdfCount.value,0);assert.equal(ebooks.calls.length,0)
})
test('copy errors are visible and a late response cannot notify another library',async()=>{
  const h=fixture([paper('one')]);const failed=h.copyPaperPdf(paper('one'));h.fail('Missing PDF');await failed
  assert.deepEqual(h.errors,['Missing PDF'])
  const next=h.copyPaperPdf(paper('one'));h.library.currentPath='/library-b';h.finish(1);await next
  assert.deepEqual(h.notices,[])
})
test('the multi-selection menu exposes the PDF count and disables the empty action',()=>{
  const multi=sfc.template.content.split('<template v-if="ctxIsMulti">')[1].split('<!-- ── Single-paper menu')[0]
  assert.match(multi,/:disabled="ctxPdfCount === 0"[^>]*@click="copyPaperPdfs\(ctxTargets\)"/)
  assert.match(multi,/t\('paper.copyPdfs', \{ n: ctxPdfCount \}\)/)
})
