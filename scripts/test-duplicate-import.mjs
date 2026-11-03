// Run: node --test scripts/test-duplicate-import.mjs
// Exercise the real import store with isolated command/dialog mocks.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { webcrypto } from 'node:crypto'
import { ref, computed } from 'vue'
import ts from 'typescript'
import { confirm as nativeConfirm } from '@tauri-apps/plugin-dialog'

const transpile = code => ts.transpileModule(code, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
const source = await readFile(new URL('../src/stores/import.ts', import.meta.url), 'utf8')
const ast = ts.createSourceFile('import.ts', source, ts.ScriptTarget.Latest, true)
const store = ast.statements.find(n => ts.isVariableStatement(n) &&
  n.declarationList.declarations.some(d => d.name.getText(ast) === 'useImportStore'))
  .declarationList.declarations[0].initializer.arguments[1].getText(ast)
const createStore = new Function('env', transpile(`
  const { ref, computed, invoke, listen, window, CustomEvent, crypto, i18n,
    useLibraryStore, useReaderStore, useSelectionStore, useCollectionsStore,
    useRanksStore, useSettingsStore, confirmDuplicateImport } = env;
  return (${store})();
`))

function fixture(accept, duplicate = true, deleteFails = false, overrides = {}) {
  const calls = [], questions = [], moves = [], renames = []
  const library = { currentPath: '/mock-library', papers: [], refreshes: 0, refresh: async () => { library.refreshes++ } }
  const selection = { selectedSlug: '', selectPaper(slug) { this.selectedSlug = slug } }
  const collections = { canReceivePapers: () => true, load: async () => {},
    movePaper: async (id, collection) => { moves.push({ id, collection }) } }
  const env = {
    ref, computed, crypto: webcrypto,
    window: { confirm() { throw new Error('Deprecated window.confirm must not be used') }, dispatchEvent() {} },
    confirmDuplicateImport: async title => { questions.push(`Import ${title} as an independent copy?`); return typeof accept === 'function' ? await accept() : await accept },
    CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail } },
    i18n: { global: { t: (_key, vars = {}) => `Import ${vars.title} as an independent copy?` } },
    listen: async () => () => {},
    useLibraryStore: () => library, useSelectionStore: () => selection,
    useCollectionsStore: () => collections,
    useReaderStore: () => ({ replacePaperSlug: (a, b) => renames.push([a, b]) }),
    useRanksStore: () => ({}), useSettingsStore: () => ({ loaded: true, easyscholarConfigured: false }),
    invoke: async (name, args) => {
      calls.push({ name, args })
      if (overrides[name]) return overrides[name](args)
      switch (name) {
        case 'delete_paper': if (deleteFails) throw new Error('delete failed'); return null
        case 'import_pdf': moves.push({ id: 'uuid-new-temp', collection: args.collectionId }); return 'new-temp'
        case 'import_ebook': return 'ebook-copy'
        case 'import_paper_url': return !args.force && duplicate
          ? { status: 'duplicate', existingSlug: 'original', title: 'Same Paper' }
          : { status: 'imported', slug: 'url-copy' }
        case 'get_paper_meta': return { id: `uuid-${args.slug}`, title: 'Same Paper' }
        case 'find_duplicate_paper': return duplicate ? { existingSlug: 'original', title: 'Same Paper' } : null
        case 'rename_paper_folder': return args.slug === 'new-temp' ? 'same-paper-2' : args.slug
        case 'extract_import_metadata': return { meta: { id: `uuid-${args.slug}`, title: 'Same Paper' }, isPaper: !args.slug.startsWith('ebook'), isBook: args.slug.startsWith('ebook') }
        default: return null
      }
    },
  }
  return { store: createStore(env), calls, questions, moves, renames, selection, library }
}
async function idle(store) {
  for (let i = 0; i < 100 && store.activeCount.value; i++) await new Promise(resolve => setImmediate(resolve))
  assert.equal(store.activeCount.value, 0, 'metadata worker finishes')
}

test('a local duplicate is kept only after approval and receives its own slug/UUID', async () => {
  const f = fixture(true)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.questions.length, 1)
  assert.match(f.questions[0], /Same Paper/)
  assert.equal(f.store.jobs.value[0].slug, 'same-paper-2')
  assert.equal(f.store.jobs.value[0].status, 'done')
  assert.deepEqual(f.moves, [{ id: 'uuid-new-temp', collection: 'leaf' }])
  assert.deepEqual(f.renames, [['new-temp', 'same-paper-2']])
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  assert.equal(f.selection.selectedSlug, 'same-paper-2')
})

test('declining a local duplicate deletes only the newly imported copy', async () => {
  const f = fixture(false)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.deepEqual(f.calls.filter(c => c.name === 'delete_paper'), [{ name: 'delete_paper', args: { slug: 'new-temp', expectedRoot: '/mock-library' } }])
  assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), false)
  assert.equal(f.selection.selectedSlug, 'original')
})

test('accepting a URL duplicate retries with force and is not asked again during metadata', async () => {
  const f = fixture(true)
  await f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  await idle(f.store)
  const imports = f.calls.filter(c => c.name === 'import_paper_url')
  assert.equal(imports.length, 2)
  assert.equal(imports[0].args.force, undefined)
  assert.equal(imports[1].args.force, true)
  assert.equal(f.questions.length, 1)
  assert.equal(f.calls.some(c => c.name === 'find_duplicate_paper' || c.name === 'delete_paper'), false)
  assert.equal(f.selection.selectedSlug, 'url-copy')
})

test('declining a URL duplicate never forces a download or deletes the existing paper', async () => {
  const f = fixture(false)
  await f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  assert.equal(f.store.jobs.value[0].status, 'skipped')
})

test('ebooks use the same choice and keep separate copies', async () => {
  for (const accept of [true, false]) {
    const f = fixture(accept)
    await f.store.importFile('/tmp/book.epub', 'leaf')
    assert.equal(f.questions.length, 1)
    assert.equal(f.calls.some(c => c.name === 'delete_paper'), !accept)
    if (accept) assert.equal(f.selection.selectedSlug, 'ebook-copy')
  }
})

test('ordinary new imports proceed without a duplicate prompt', async () => {
  const f = fixture(false, false)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.questions.length, 0)
  assert.equal(f.store.jobs.value[0].status, 'done')
})

const deferred = () => {
  let resolve
  const promise = new Promise(done => { resolve = done })
  return { promise, resolve }
}
async function until(predicate) {
  for (let i = 0; i < 100 && !predicate(); i++) await new Promise(resolve => setImmediate(resolve))
  assert.ok(predicate())
}

test('a local import pauses until the actual asynchronous duplicate choice arrives', async () => {
  for (const accept of [false, true]) {
    const choice = deferred(), f = fixture(choice.promise)
    await f.store.importFile('/tmp/paper.pdf', 'leaf')
    await until(() => f.questions.length === 1)
    assert.ok(f.store.activeCount.value > 0)
    assert.equal(f.calls.some(c => ['rename_paper_folder', 'fetch_citation_count', 'delete_paper'].includes(c.name)), false)
    choice.resolve(accept)
    await idle(f.store)
    assert.equal(f.calls.some(c => c.name === 'delete_paper'), !accept)
    assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), accept)
  }
})

test('URL duplicates cannot force a copy before the asynchronous user choice', async () => {
  const choice = deferred(), f = fixture(choice.promise)
  const importing = f.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  await until(() => f.questions.length === 1)
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  choice.resolve(false)
  await importing
  assert.equal(f.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(f.store.jobs.value[0].status, 'skipped')
})

test('a dialog failure never silently approves a known local or URL duplicate', async () => {
  const fail = () => { throw new Error('dialog failed') }
  const local = fixture(fail)
  await local.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(local.store)
  assert.equal(local.store.jobs.value[0].status, 'error')
  assert.equal(local.calls.some(c => ['rename_paper_folder', 'fetch_citation_count', 'delete_paper'].includes(c.name)), false)
  const url = fixture(fail)
  await url.store.importPaperUrl('https://arxiv.org/abs/2401.12345', 'leaf')
  assert.equal(url.calls.filter(c => c.name === 'import_paper_url').length, 1)
  assert.equal(url.store.jobs.value[0].status, 'error')
  const ebook = fixture(fail)
  await ebook.store.importFile('/tmp/book.epub', 'leaf')
  assert.equal(ebook.store.jobs.value[0].status, 'error')
  assert.equal(ebook.store.activeCount.value, 0)
  assert.equal(ebook.calls.some(c => c.name === 'delete_paper'), false)
})

test('a failed cleanup is reported as an error and never claims the import was skipped or deletes the original', async () => {
  const f = fixture(false, true, true)
  await f.store.importFile('/tmp/paper.pdf', 'leaf')
  await idle(f.store)
  assert.equal(f.store.jobs.value[0].status, 'error')
  assert.equal(f.store.lastDuplicate.value, null)
  assert.deepEqual(f.calls.filter(c => c.name === 'delete_paper'), [{ name: 'delete_paper', args: { slug: 'new-temp', expectedRoot: '/mock-library' } }])
  assert.equal(f.calls.some(c => c.name === 'rename_paper_folder'), false)
})

test('the real dialog helper uses the supported native command and localized keep/skip buttons', async () => {
  const helper = await readFile(new URL('../src/utils/duplicateImport.ts', import.meta.url), 'utf8')
  const helperAst = ts.createSourceFile('duplicateImport.ts', helper, ts.ScriptTarget.Latest, true)
  const fn = helperAst.statements.filter(n => !ts.isImportDeclaration(n)).map(n => n.getText(helperAst).replace(/^export /, '')).join('\n')
  const translations = { 'import.duplicateConfirm': 'Same-title paper: {title}', 'import.duplicateTitle': '论文可能重复',
    'import.keepDuplicate': '保留副本', 'import.skipDuplicate': '跳过导入' }
  const confirmDuplicate = new Function('confirm', 'i18n', transpile(`${fn}\nreturn confirmDuplicateImport;`))(
    nativeConfirm, { global: { t: (key, args = {}) => translations[key].replace('{title}', args.title ?? '') } })
  const previousWindow = globalThis.window
  try {
    for (const accept of [true, false]) {
      const choice = deferred(), calls = []
      globalThis.window = { confirm() { throw new Error('Broken shim') }, __TAURI_INTERNALS__: {
        invoke(command, args) { calls.push({ command, args }); return choice.promise },
      } }
      const result = confirmDuplicate('Same Paper')
      await until(() => calls.length === 1)
      assert.equal(calls[0].command, 'plugin:dialog|message')
      assert.equal(calls[0].args.title, '论文可能重复')
      assert.ok(JSON.stringify(calls[0].args.buttons).includes('保留副本'))
      assert.ok(JSON.stringify(calls[0].args.buttons).includes('跳过导入'))
      choice.resolve(accept ? '保留副本' : '跳过导入')
      assert.equal(await result, accept)
    }
  } finally { globalThis.window = previousWindow }
})

function batchFixture(accept = false, extra = {}) {
  return fixture(accept, false, false, {
    import_pdf: async args => `pdf-${args.sourcePath.split('/').pop()}`,
    import_ebook: async args => `ebook-${args.sourcePath.split('/').pop()}`,
    extract_import_metadata: async args => ({ meta: { id: args.slug, title: args.slug }, isPaper: args.slug.startsWith('pdf-'), isBook: args.slug.startsWith('ebook-') }),
    rename_paper_folder: async args => `final-${args.slug}`,
    ...extra,
  })
}

test('all fifteen mixed files are registered immediately, a broken book is reported, and later files still import', async () => {
  const f = batchFixture(false, { import_ebook: async args => {
    if (args.sourcePath.endsWith('7.epub')) throw new Error('Damaged EPUB')
    return `ebook-${args.sourcePath.split('/').pop()}`
  } })
  const paths = Array.from({ length: 15 }, (_, i) => `/tmp/${i}.${i % 2 ? 'epub' : 'pdf'}`)
  const pending = f.store.importFiles(paths, 'leaf')
  assert.equal(f.store.jobs.value.length, 15)
  await pending; await idle(f.store)
  assert.equal(f.calls.filter(c => ['import_pdf','import_ebook'].includes(c.name)).length, 15)
  assert.equal(f.store.jobs.value.filter(j => j.status === 'done').length, 14)
  const error = f.store.jobs.value.find(j => j.filename === '7.epub')
  assert.equal(error.status, 'error'); assert.match(error.error, /Damaged EPUB/)
  assert.equal(f.store.jobs.value.find(j => j.filename === '14.pdf').status, 'done')
  assert.equal(new Set(f.store.jobs.value.map(j => j.id)).size, 15)
})

test('one pending duplicate decision does not prevent every remaining ebook from being copied', async () => {
  const choice = deferred()
  const f = batchFixture(choice.promise, { find_duplicate_paper: async () => ({ existingSlug: 'original', title: 'Same title' }) })
  const pending = f.store.importFiles(Array.from({ length: 16 }, (_, i) => `/tmp/${i}.epub`), 'leaf')
  await until(() => f.questions.length === 1)
  assert.equal(f.calls.filter(c => c.name === 'import_ebook').length, 16)
  assert.equal(f.store.jobs.value.length, 16)
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  choice.resolve(false); await pending; await idle(f.store)
  assert.equal(f.calls.filter(c => c.name === 'delete_paper').length, 16)
  assert.equal(f.store.jobs.value.every(j => j.status === 'skipped'), true)
})

test('overlapping drops share the same two local IO slots and retain every queued file', async () => {
  const holds = []; let active = 0, maximum = 0
  const f = batchFixture(false, { import_ebook: args => {
    active++; maximum = Math.max(maximum, active)
    return new Promise(resolve => holds.push(() => { active--; resolve(`ebook-${args.sourcePath.split('/').pop()}`) }))
  } })
  const a = f.store.importFiles(Array.from({ length: 9 }, (_, i) => `/tmp/a-${i}.epub`), 'leaf')
  const b = f.store.importFiles(Array.from({ length: 8 }, (_, i) => `/tmp/b-${i}.epub`), 'leaf')
  assert.equal(f.store.jobs.value.length, 17)
  assert.equal(maximum, 2)
  for (let i = 0; i < 100 && f.store.activeCount.value; i++) {
    holds.splice(0).forEach(release => release())
    await new Promise(resolve => setImmediate(resolve))
  }
  await Promise.all([a,b]); await idle(f.store)
  assert.equal(maximum, 2)
  assert.equal(f.store.jobs.value.filter(j => j.status === 'done').length, 17)
})

test('same-batch copies cannot point at undecided copies and skipping does not remove every original', async () => {
  const retained = new Set()
  const f = batchFixture(false, {
    import_ebook: async args => { const slug = `ebook-${args.sourcePath.split('/').pop()}`; retained.add(slug); return slug },
    rename_paper_folder: async args => args.slug,
    find_duplicate_paper: async args => {
      const original = [...retained].find(slug => slug !== args.slug && !args.excludeSlugs.includes(slug))
      return original ? { existingSlug: original, title: 'Same book' } : null
    },
    delete_paper: async args => retained.delete(args.slug),
  })
  await f.store.importFiles(['/tmp/a.epub','/tmp/b.epub','/tmp/c.epub'], 'leaf'); await idle(f.store)
  assert.equal(retained.size, 1)
  assert.equal(f.store.jobs.value.filter(j => j.status === 'done').length, 1)
  assert.equal(f.store.jobs.value.filter(j => j.status === 'skipped').length, 2)
  assert.equal(f.questions.length, 2)
})

test('full-book extraction and slow citations do not block the next PDFs metadata or process skipped PDFs', async () => {
  const text = deferred()
  const f = batchFixture(false, { extract_fulltext: () => text.promise })
  await f.store.importFiles(Array.from({ length: 12 }, (_, i) => `/tmp/${i}.pdf`), 'leaf')
  await until(() => f.calls.filter(c => c.name === 'rename_paper_folder').length === 12)
  assert.equal(f.calls.filter(c => c.name === 'extract_import_metadata').length, 12)
  assert.equal(f.calls.filter(c => c.name === 'extract_fulltext').length, 1)
  text.resolve(); await idle(f.store)
  const duplicate = batchFixture(false, { find_duplicate_paper: async () => ({ existingSlug: 'old', title: 'Existing' }) })
  await duplicate.store.importFile('/tmp/copy.pdf', 'leaf'); await idle(duplicate.store)
  assert.equal(duplicate.calls.some(c => c.name === 'extract_fulltext'), false)
})

test('recognized PDF books and ebooks never enter scholarly venue/reference enrichment', async () => {
  const f = batchFixture(false, { extract_import_metadata: async args => ({ meta: { id: args.slug, title: 'Book' }, isPaper: false, isBook: true }) })
  await f.store.importFiles(['/tmp/a.pdf','/tmp/b.epub'], 'leaf'); await idle(f.store)
  assert.equal(f.calls.some(c => ['fetch_metadata','fetch_citation_count','fetch_references'].includes(c.name)), false)
  assert.equal(f.store.jobs.value.every(j => j.status === 'done'), true)
})

test('metadata failure retains the file with a visible warning and disallows loose title-search fallback', async () => {
  const f = batchFixture(false, {
    extract_import_metadata: async () => { throw new Error('Uncertain title after ten pages') },
    fetch_metadata: async args => ({ id: args.slug, title: 'Original filename' }),
  })
  await f.store.importFiles(['/tmp/a.pdf','/tmp/b.epub'], 'leaf'); await idle(f.store)
  assert.equal(f.store.jobs.value.every(j => j.status === 'done' && j.warning), true)
  assert.equal(f.calls.find(c => c.name === 'fetch_metadata').args.allowTitleSearch, false)
  assert.equal(f.calls.some(c => ['fetch_citation_count','fetch_references'].includes(c.name)), false)
})

test('switching libraries while a duplicate prompt is pending never deletes from the new library', async () => {
  const choice = deferred()
  const f = batchFixture(choice.promise, { find_duplicate_paper: async () => ({ existingSlug: 'original', title: 'Same' }) })
  const pending = f.store.importFile('/tmp/a.epub', 'leaf')
  await until(() => f.questions.length === 1)
  f.library.currentPath = '/different-library'
  choice.resolve(false); await pending; await idle(f.store)
  assert.equal(f.store.jobs.value[0].status, 'error')
  assert.equal(f.calls.some(c => c.name === 'delete_paper'), false)
  assert.equal(f.calls.every(c => c.args?.expectedRoot === '/mock-library'), true)
})

test('queued files stop after a library switch without silently importing into another library', async () => {
  const copied = deferred()
  const f = batchFixture(false, { import_ebook: async args => { await copied.promise; return `ebook-${args.sourcePath.split('/').pop()}` } })
  const pending = f.store.importFiles(Array.from({ length: 8 }, (_, i) => `/tmp/${i}.epub`), 'leaf')
  f.library.currentPath = '/different-library'; copied.resolve()
  await pending; await idle(f.store)
  assert.equal(f.calls.filter(c => c.name === 'import_ebook').length, 2)
  assert.equal(f.store.jobs.value.length, 8)
  assert.equal(f.store.jobs.value.every(j => j.status === 'error'), true)
})

test('skipping binds cleanup to the exact incoming UUID, with visible progress while deletion is pending', async () => {
  const deletion = deferred()
  const f = batchFixture(false, {
    find_duplicate_paper: async () => ({ existingSlug:'original',title:'Same',incomingId:'new-copy-uuid' }),
    delete_paper: () => deletion.promise,
  })
  const pending=f.store.importFile('/tmp/a.epub','leaf')
  await until(()=>f.calls.some(c=>c.name==='delete_paper'))
  assert.equal(f.store.jobs.value[0].status,'skipping')
  assert.equal(f.calls.find(c=>c.name==='delete_paper').args.expectedId,'new-copy-uuid')
  deletion.resolve();await pending;await idle(f.store)
  assert.equal(f.store.jobs.value[0].status,'skipped')
})
