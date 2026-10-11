// Run: node --test scripts/test-speech.mjs. No provider, browser or API key is used.
import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import ts from 'typescript'
import { createPinia, disposePinia } from 'pinia'

const dir = await mkdtemp(join(tmpdir(), 'argus-speech-test-'))
after(() => rm(dir, { recursive: true, force: true }))
for (const name of ['speechEngine', 'speechPages', 'speechText', 'speechFollow', 'highlightText']) {
  const source = await readFile(new URL(`../src/utils/${name}.ts`, import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
  })
  await writeFile(join(dir, `${name}.mjs`), outputText.replace(/from '(.\/[^']+)'/g, "from '$1.mjs'"))
}
const { SpeechEngine, SILENT_WAV } = await import(pathToFileURL(join(dir, 'speechEngine.mjs')))
const { SpeechPages, pdfSpeechText } = await import(pathToFileURL(join(dir, 'speechPages.mjs')))

// Exercise the real store with local settings and fake Tauri/audio I/O. In
// particular, seeking must retain an ebook read's identity and frozen voice.
await writeFile(join(dir, 'speechStoreMocks.mjs'), `
let current
export const configure = mocks => { current = mocks }
export const invoke = (...args) => current.invoke(...args)
export const useSettingsStore = () => current.settings
export const useAiStore = () => ({ settings: { providers: [] } })
export const i18n = { global: { t: key => key } }
`)
const storeSource = await readFile(new URL('../src/stores/speech.ts', import.meta.url), 'utf8')
const storeJs = ts.transpileModule(storeSource, {
  compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
}).outputText
await writeFile(join(dir, 'speechStore.mjs'), storeJs
  .replace(/from 'pinia'/g, `from '${import.meta.resolve('pinia')}'`)
  .replace(/from 'vue'/g, `from '${import.meta.resolve('vue')}'`)
  .replace(/from '(?:@tauri-apps\/api\/core|\.\.\/i18n|\.\/settings|\.\/ai)'/g, "from './speechStoreMocks.mjs'")
  .replace(/from '\.\.\/utils\/([^']+)'/g, "from './$1.mjs'"))
const { configure } = await import(pathToFileURL(join(dir, 'speechStoreMocks.mjs')))
const { useSpeechStore } = await import(pathToFileURL(join(dir, 'speechStore.mjs')))
const flush = async () => { for (let i = 0; i < 40; i++) await Promise.resolve() }
const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const clip = text => ({ dataUrl: `data:audio/wav;base64,${Buffer.from(text).toString('base64')}`, mime: 'audio/wav' })

class FakeAudio {
  _src = ''; currentTime = 0; duration = 10; readyState = 0
  playbackRate = 1; defaultPlaybackRate = 1; volume = 1; playing = false
  listeners = new Map(); loads = []; plays = []
  get src() { return this._src }
  set src(v) { this._src = v; this.currentTime = 0; this.readyState = 0 }
  addEventListener(name, fn) {
    if (!this.listeners.has(name)) this.listeners.set(name, new Set())
    this.listeners.get(name).add(fn)
  }
  removeEventListener(name, fn) { this.listeners.get(name)?.delete(fn) }
  emit(name) { for (const fn of [...(this.listeners.get(name) ?? [])]) fn() }
  load() { this.loads.push(this.src); this.readyState = 1; this.emit('loadedmetadata') }
  play() {
    this.plays.push(this.src); this.playing = true
    if (!this.readyState) { this.readyState = 1; this.emit('loadedmetadata') }
    return Promise.resolve()
  }
  pause() { this.playing = false }
  removeAttribute() { this.src = '' }
  end() { this.playing = false; this.currentTime = this.duration; this.emit('ended') }
}

function harness(synthesize, extra = {}) {
  const audios = [], requests = [], events = []
  const engine = new SpeechEngine({
    createAudio: () => { const audio = new FakeAudio(); audios.push(audio); return audio },
    synthesize: async (text, signal, scope) => {
      requests.push({ text, signal, scope })
      return synthesize ? synthesize(text, signal, scope) : clip(text)
    },
    onState: (state, info) => events.push({ state, ...info }),
    wait: async () => {}, ...extra,
  })
  const active = () => audios.find(a => a.playing && a.src !== SILENT_WAV)
  return { engine, audios, requests, events, active }
}

function storeHarness(t, patch = {}) {
  const audios = [], requests = []
  // Playback positions are sampled on state changes and seeks. Do not schedule
  // a perpetual animation loop in this Node test process.
  globalThis.requestAnimationFrame = () => 1
  globalThis.cancelAnimationFrame = () => {}
  globalThis.localStorage = { getItem: () => null, setItem: () => {} }
  globalThis.Audio = class extends FakeAudio {
    constructor() { super(); audios.push(this) }
  }
  const settings = { settings: {
    speech_provider_id: 'provider', speech_model_id: 'voice',
    speech_options: {}, speech_skip_citations: true, ...patch,
  } }
  configure({ settings, invoke: async (name, args) => {
    requests.push({ name, args })
    if (name === 'list_media_capabilities') return [{
      providerId: 'provider', capabilities: [{ kind: 'speech', models: [{ id: 'voice', maxPromptChars: 80, fields: [] }] }],
    }]
    if (name === 'read_speech_audio' || name === 'save_speech_audio') return null
    if (name === 'run_media_task') return { artifacts: [clip(args.request.prompt)] }
    throw new Error(`Unexpected command: ${name}`)
  } })
  const pinia = createPinia()
  const store = useSpeechStore(pinia)
  t.after(() => { store.stop(); disposePinia(pinia) })
  return {
    store, settings, audios, requests,
    active: () => audios.find(a => a.playing && a.src !== SILENT_WAV),
    synths: () => requests.filter(r => r.name === 'run_media_task').map(r => r.args.request),
  }
}

test('ebook continuous read unlocks audio, loads chapters lazily, skips blanks and finishes', async t => {
  const h = storeHarness(t), fetched = []
  const read = h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 3, endPage: 6,
    getText: async chapter => { fetched.push(chapter); return chapter === 4 ? '' : `章节 ${chapter}。` + '正文有文献引用 [12]。'.repeat(40) },
  })
  assert(h.audios.length === 2 && h.audios.every(a => a.plays.includes(SILENT_WAV)))
  assert(h.store.isReadingPaper('book')) // Also true while the first local text is pending.
  assert(!h.store.isReadingPaper('other-book'))
  await read; await flush()
  assert.deepEqual(fetched, [3])
  assert.equal(h.store.activeSource, 'ebook-chapters')
  assert.equal(h.store.pageProgress.unit, 'chapter')
  assert.equal(h.store.pageProgress.page, 3)
  for (let i = 0; i < 100 && h.store.state !== 'idle'; i++) {
    assert(h.active(), 'continuous playback must have an active clip')
    h.active().end(); await flush()
  }
  assert.equal(h.store.state, 'idle')
  assert.deepEqual(fetched, [3, 4, 5, 6])
  assert(h.synths().every(r => Array.from(r.prompt).length <= 80 && !r.prompt.includes('[12]')))
  assert.equal(h.store.pageProgress, null)
  assert(!h.store.isReadingPaper('book'))
})

test('ebook seek loads only its target chapter, retains pause state and uses the frozen voice', async t => {
  const h = storeHarness(t), fetched = []
  await h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 3, endPage: 10,
    getText: async chapter => { fetched.push(chapter); return `Chapter ${chapter}. ` + 'A sentence. '.repeat(60) },
  }); await flush()
  h.store.setRate(1.5); h.store.setVolume(0.3); h.store.pause()
  h.settings.settings.speech_model_id = 'changed-voice'
  h.store.seekTo(4.5 / 8); await flush()
  assert.deepEqual(fetched, [3, 7])
  assert.equal(h.store.state, 'paused')
  assert(h.store.isReadingPaper('book'))
  assert.equal(h.store.activeSource, 'ebook-chapters')
  assert.equal(h.store.pageProgress.unit, 'chapter')
  assert.equal(h.store.pageProgress.page, 7)
  assert(Math.abs(h.store.heard - 4.5 / 8) < 0.001)
  assert(h.synths().every(r => r.model === 'voice'))
  assert(h.audios.every(a => a.playbackRate === 1.5 && a.volume === 0.3))
  h.store.resume(); await flush()
  assert.equal(h.store.state, 'playing')
  assert(h.active())
})

test('closing an ebook while its text is loading prevents late audio and further chapter loads', async t => {
  const h = storeHarness(t), pending = deferred(), fetched = []
  const read = h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 2, endPage: 100,
    getText: chapter => { fetched.push(chapter); return pending.promise },
  })
  await flush()
  if (h.store.isReadingPaper('book')) h.store.stop() // The viewer's close-tab action.
  pending.resolve('Late chapter text.'); await read; await flush()
  assert.equal(h.store.state, 'idle')
  assert.deepEqual(fetched, [2])
  assert.deepEqual(h.synths(), [])
  assert(h.audios.every(a => !a.src || a.src === SILENT_WAV))
})

test('clicking an ebook read toggle again stops it before the text arrives', async t => {
  const h = storeHarness(t), pending = deferred()
  const options = { paper: 'book', unit: 'chapter', startPage: 1, endPage: 2, getText: () => pending.promise }
  const read = h.store.readPages(options); await flush()
  await h.store.readPages(options)
  pending.resolve('Late text.'); await read; await flush()
  assert.equal(h.store.state, 'idle')
  assert.deepEqual(h.synths(), [])
})

test('unconfigured ebook reading opens voice setup without fetching text or synthesizing', async t => {
  const h = storeHarness(t, { speech_provider_id: '', speech_model_id: '' })
  await h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 1, endPage: 2,
    getText: () => { assert.fail('text must not load before a voice is chosen') },
  }); await flush()
  assert.equal(h.store.state, 'idle')
  assert.equal(h.store.setupPrompt.reason, 'unset')
  assert.deepEqual(h.synths(), [])
})

test('empty ebooks and chapter load failures show ebook-specific notices', async t => {
  const h = storeHarness(t)
  await h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 1, endPage: 2, getText: async () => '' })
  assert.equal(h.store.state, 'error')
  assert.equal(h.store.errorMessage, 'speech.errors.chaptersEmpty')
  h.store.stop()
  await h.store.readPages({ paper: 'book', unit: 'chapter', startPage: 1, endPage: 2,
    getText: async () => { throw new Error('chapter unavailable') },
  })
  assert.equal(h.store.state, 'error')
  assert.equal(h.store.errorMessage, 'speech.errors.ebookText')
  assert.deepEqual(h.synths(), [])
})

test('PDF continuous reads keep page progress and PDF notices by default', async t => {
  const h = storeHarness(t)
  await h.store.readPages({ paper: 'pdf', startPage: 2, endPage: 3, getText: async () => 'A sentence. '.repeat(60) })
  await flush()
  assert.equal(h.store.activeSource, 'pdf-pages')
  assert.equal(h.store.pageProgress.unit, 'page')
  assert(h.store.isReadingPaper('pdf'))
  h.store.stop()
  await h.store.readPages({ paper: 'pdf', startPage: 2, endPage: 3, getText: async () => '' })
  assert.equal(h.store.errorMessage, 'speech.errors.pagesEmpty')
})

test('PDF text preserves line ends, separates words and ignores marked-content records', () => {
  assert.equal(pdfSpeechText([{ str: 'One' }, { str: 'two.', hasEOL: true }, { type: 'begin' }, { str: 'Three.' }]), 'One two.\nThree. ')
})

test('pages start at the requested page, skip blanks, keep order and respect API character limits', async () => {
  const fetched = []
  const input = new SpeechPages({ startPage: 3, endPage: 5, maxChars: 80, skipCitations: true,
    cancelled: () => false, getText: async page => { fetched.push(page); return page === 4 ? '' : `Page ${page}. ` + 'A sentence [12]. '.repeat(40) },
  })
  await input.next()
  assert.deepEqual(fetched, [3])
  assert(input.chunks.every(c => Array.from(c).length <= 80 && !c.includes('[12]')))
  await input.next()
  assert.deepEqual(fetched, [3, 4, 5])
  assert.deepEqual(input.pages.map(p => p.page), [3, 5])
  assert.equal(await input.next(), null)
  const span = input.pages[1]
  assert.equal(input.position(span.start, 0).page, 5)
  assert.equal(input.position(span.end - 1, 1).fraction, 1)
  assert(input.target(5, 0.5).index >= span.start)
  assert.equal(input.target(4, 0.5), null)
})

test('stopping during page extraction discards its text and prevents later page reads', async () => {
  const pending = deferred(); let cancelled = false; const fetched = []
  const input = new SpeechPages({ startPage: 2, endPage: 100, maxChars: 500, skipCitations: false,
    cancelled: () => cancelled, getText: page => { fetched.push(page); return pending.promise },
  })
  const next = input.next(); cancelled = true; pending.resolve('Late text.')
  assert.equal(await next, null)
  assert.equal(await input.next(), null)
  assert.deepEqual(fetched, [2]); assert.deepEqual(input.chunks, [])
})

test('one clip ahead is synthesized and preloaded on a second unlocked player', async () => {
  const h = harness(); h.engine.unlock()
  assert.equal(h.audios.length, 2)
  assert(h.audios.every(a => a.plays.includes(SILENT_WAV)))
  h.engine.read(['first', 'second', 'third'], 'voice'); await flush()
  assert.deepEqual(h.requests.map(r => r.text), ['first', 'second'])
  const first = h.active(), second = h.audios.find(a => a !== first)
  assert(second.loads.includes(clip('second').dataUrl))
  assert(!second.plays.includes(clip('second').dataUrl))
  first.end(); await flush()
  assert.equal(h.active(), second)
  assert.deepEqual(h.requests.map(r => r.text), ['first', 'second', 'third'])
  second.end(); await flush(); h.active().end(); await flush()
  assert.equal(h.engine.state, 'idle')
  assert(h.audios.every(a => !a.playing && !a.src))
})

test('lazy batches are pulled only at the page boundary and continue to the end', async () => {
  let pulls = 0
  const h = harness()
  h.engine.read(['one', 'two', 'three'], 'v', { next: async () => ++pulls === 1 ? ['four', 'five'] : null })
  await flush(); assert.equal(pulls, 0)
  h.active().end(); await flush(); assert.equal(pulls, 0)
  h.active().end(); await flush(); assert.equal(pulls, 1)
  assert.deepEqual(h.requests.map(r => r.text), ['one', 'two', 'three', 'four'])
  h.active().end(); await flush(); h.active().end(); await flush(); h.active().end(); await flush()
  assert.equal(h.engine.state, 'idle'); assert.equal(pulls, 2)
})

test('cancelled boundary extraction cannot synthesize or play the late next page', async () => {
  const pending = deferred(); const h = harness()
  h.engine.read(['one'], 'v', { next: () => pending.promise }); await flush()
  h.engine.stop(); pending.resolve(['late']); await flush()
  assert.equal(h.engine.state, 'idle')
  assert.deepEqual(h.requests.map(r => r.text), ['one'])
  assert(h.audios.every(a => !a.playing && !a.src))
})

test('a late prefetch from a replaced read cannot overwrite its new prepared audio', async () => {
  const pending = deferred(); const h = harness(text => text === 'old-next' ? pending.promise : clip(text))
  h.engine.read(['old', 'old-next'], 'v'); await flush()
  h.engine.read(['new', 'new-next'], 'v'); await flush()
  pending.resolve(clip('old-next')); await flush()
  h.active().end(); await flush()
  assert.equal(h.active().src, clip('new-next').dataUrl)
  h.engine.stop()
})

test('pause, resume, speed, volume and seeking work with the alternating players', async () => {
  const h = harness(); h.engine.read(['one', 'two', 'three'], 'v'); await flush()
  h.engine.setRate(1.5); h.engine.setVolume(0.3)
  assert(h.audios.every(a => a.playbackRate === 1.5 && a.volume === 0.3))
  const first = h.active(); first.currentTime = 4
  assert(h.engine.pause()); assert.equal(h.engine.position().fraction, 0.4)
  assert(h.engine.seek(2, 0.6)); await flush()
  assert.equal(h.engine.state, 'paused')
  assert.equal(h.engine.position().index, 2)
  assert.equal(h.engine.position().fraction, 0.6)
  assert(h.engine.resume()); await flush()
  assert.equal(h.active().currentTime, 6)
  h.active().end(); await flush(); assert.equal(h.engine.state, 'idle')
})

test('seeking while a lazy page is pending neither pulls it twice nor drops its chunks', async () => {
  const pending = deferred(); let pulls = 0; const h = harness()
  h.engine.read(['one'], 'v', { next: () => ++pulls === 1 ? pending.promise : Promise.resolve(null) }); await flush()
  h.engine.seek(0, 0.5); await flush()
  assert.equal(pulls, 1)
  pending.resolve(['two']); await flush()
  h.active().end(); await flush(); assert.equal(h.active().src, clip('two').dataUrl)
  h.active().end(); await flush(); assert.equal(h.engine.state, 'idle')
  assert.equal(h.requests.filter(r => r.text === 'two').length, 1)
})

test('a page error is reported after the current clip; no unhandled prefetch rejection', async () => {
  const h = harness()
  h.engine.read(['one'], 'v', { next: async () => { throw new Error('cannot extract page') } }); await flush()
  assert.equal(h.engine.state, 'playing')
  h.active().end(); await flush()
  assert.equal(h.engine.state, 'error')
  assert.match(h.events.at(-1).error.message, /cannot extract page/)
})

test('retry is bounded, cancellation blocks retries, and replay uses the voice cache', async () => {
  let attempts = 0
  const h = harness(async text => { if (++attempts === 1) throw new Error('API error 503'); return clip(text) })
  h.engine.read(['one'], 'v'); await flush(); assert.equal(attempts, 2)
  h.active().end(); await flush()
  h.engine.read(['one'], 'v'); await flush(); assert.equal(attempts, 2)
  h.engine.stop()
  const wait = deferred()
  const retry = harness(async () => { throw new Error('API error 503') }, { wait: () => wait.promise })
  retry.engine.read(['one'], 'v'); await flush(); retry.engine.stop(); wait.resolve(); await flush()
  assert.equal(retry.requests.length, 1); assert.equal(retry.engine.state, 'idle')
})

test('long reads retain only bounded audio cache and release completed request references', async () => {
  const h = harness(undefined, { maxCacheEntries: 3, maxCacheBytes: 1000 })
  h.engine.read(Array.from({ length: 40 }, (_, i) => `chunk ${i}`), 'v')
  for (let i = 0; i < 40; i++) {
    await flush()
    assert(h.engine.cacheSize <= 3)
    assert.equal(h.engine.run.held.size, 0)
    h.active().end()
  }
  await flush(); assert.equal(h.engine.state, 'idle')
})
