/**
 * Read-aloud playback: turns a list of text chunks into one continuous stretch of
 * audio, one speech request at a time.
 *
 * Pure TypeScript with everything environmental injected — the synthesiser (a
 * Tauri `invoke` in the app), the audio element (`new Audio()` in the app) and
 * the state callback — so Node can drive it with fakes. No Vue, no Tauri, no DOM.
 *
 * What it guarantees, and why each matters here:
 *
 * - ONE persistent audio element, reused for every chunk by swapping `src`.
 *   Older WebKits decide whether sound may start per element, so a fresh element
 *   per chunk would need a fresh user gesture each time and the second chunk
 *   could be silent; one element is unlocked once and plays them all.
 * - Synthesis is billed per character, so nothing is requested twice: a finished
 *   chunk goes into an LRU cache, an in-flight request is shared by whoever asks
 *   for the same text next, and a stopped-then-restarted selection costs nothing
 *   for the part that already came back.
 * - ONE chunk ahead: chunk i+1 is requested while chunk i plays, never further, so
 *   stopping early wastes at most one chunk, and the provider never sees more than
 *   one request from us at a time.
 * - A generation token on every run: `stop()` and a new `read()` invalidate
 *   everything in flight, so a late result can never start playing under a newer
 *   selection (it is still cached — it was paid for).
 * - Errors are never silent. A play() the browser refuses is an `error` state with
 *   its own reason, and an auth / quota / length error stops at once with the
 *   provider's own message, retrying only what is plausibly transient.
 */

// ── Types ─────────────────────────────────────────────────────────────────────

export type SpeechState = 'idle' | 'loading' | 'playing' | 'paused' | 'error'

/** What went wrong, as a code the UI words in the user's language. */
export type SpeechErrorReason =
  /** The provider refused or failed; `message` is its own text. */
  | 'synth'
  /** The WebView refused to start playback (autoplay policy). */
  | 'autoplay'
  /** The audio arrived but could not be decoded. */
  | 'decode'
  /** Playback was interrupted or failed for another reason. */
  | 'playback'

export interface SpeechInfo {
  /** The chunk being loaded / played, zero-based. For an error: the chunk that failed. */
  index: number
  total: number
  /** Present on `idle`: whether the text was read to the end or stopped early. */
  reason?: 'finished' | 'stopped'
  /** Present on `error`. */
  error?: { reason: SpeechErrorReason; message: string; fatal: boolean }
}

/** Handed to the synthesiser so a request nobody wants any more can say so. */
export interface SynthSignal {
  cancelled: boolean
}

export interface SynthResult {
  dataUrl: string
  mime: string
}

/** The slice of `HTMLAudioElement` the engine uses — and a fake can implement. */
export interface AudioLike {
  src: string
  play(): Promise<void> | void
  pause(): void
  addEventListener(type: string, listener: () => void): void
  removeEventListener(type: string, listener: () => void): void
  removeAttribute?(name: string): void
  load?(): void
}

export interface SpeechEngineDeps {
  /**
   * `scope` is the one the run was started with (see `speechScope`). It is passed
   * back so a synthesiser that serves several voices can tell which one THIS
   * request belongs to: a retry or a late prefetch of an older read must not pick
   * up the settings of a newer one and cache the wrong voice under the old key.
   */
  synthesize(text: string, signal: SynthSignal, scope: string): Promise<SynthResult>
  createAudio(): AudioLike
  onState(state: SpeechState, info: SpeechInfo): void
  /** Chunks kept for replay. Default 20. */
  maxCacheEntries?: number
  /**
   * Ceiling on what the kept chunks may add up to, in characters of their data
   * URLs (about a byte each). Default 48 million. A 900-character passage is a
   * few hundred kB as MP3 but several MB as WAV or FLAC, so the entry count alone
   * would let twenty of them sit in memory.
   */
  maxCacheBytes?: number
  /** Pause before the single retry of a transient failure. Default 1500 ms. */
  retryDelayMs?: number
  /** Injected so tests do not wait in real time. */
  wait?(ms: number): Promise<void>
}

export class SpeechEngineError extends Error {
  reason: SpeechErrorReason
  fatal: boolean
  constructor(reason: SpeechErrorReason, message: string, fatal: boolean) {
    super(message)
    this.name = 'SpeechEngineError'
    this.reason = reason
    this.fatal = fatal
  }
}

// ── Cache keys ────────────────────────────────────────────────────────────────

/** `JSON.stringify` with object keys sorted, so `{a:1,b:2}` and `{b:2,a:1}` are one key. */
export function stableStringify(value: unknown): string {
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'null'
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`
  const obj = value as Record<string, unknown>
  const parts: string[] = []
  for (const k of Object.keys(obj).sort()) {
    if (obj[k] === undefined) continue
    parts.push(`${JSON.stringify(k)}:${stableStringify(obj[k])}`)
  }
  return `{${parts.join(',')}}`
}

/** Everything about a request except the text: provider, model and the knobs. */
export function speechScope(providerId: string, model: string, options: Record<string, unknown>): string {
  return `${providerId}|${model}|${stableStringify(options)}`
}

/** cyrb53: a small, stable, well-mixed 53-bit string hash. */
function cyrb53(str: string, seed: number): number {
  let h1 = 0xdeadbeef ^ seed
  let h2 = 0x41c6ce57 ^ seed
  for (let i = 0; i < str.length; i++) {
    const ch = str.charCodeAt(i)
    h1 = Math.imul(h1 ^ ch, 2654435761)
    h2 = Math.imul(h2 ^ ch, 1597334677)
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507)
  h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909)
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507)
  h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909)
  return 4294967296 * (2097151 & h2) + (h1 >>> 0)
}

/**
 * The cache key for one chunk: a stable hash of `scope|text`.
 *
 * Two independent 53-bit hashes plus the length — a collision would play the
 * wrong sentence, so the key is made far longer than the cache is big. Stable
 * means identical across runs, option-key order and sessions, never random.
 */
export function speechCacheKey(scope: string, text: string): string {
  const s = `${scope}\u0001${text}`
  return `${cyrb53(s, 0).toString(36)}${cyrb53(s, 0x9e3779b9).toString(36)}.${s.length}`
}

// ── Error classification ──────────────────────────────────────────────────────

/**
 * Whether a failed request is worth one more try.
 *
 * Only what is plausibly momentary is: timeouts, dropped connections, 5xx, rate
 * limiting. Auth, balance, quota, over-length and unsupported-input errors are
 * `fatal` — repeating them changes nothing and (for a billing error) annoys the
 * provider. An error we do not recognise is also `fatal`: a retry is a second
 * bill if the first request got further than it looked.
 *
 * The provider's own message is always kept and shown; this only decides retry.
 */
export function classifySpeechError(err: unknown): 'transient' | 'fatal' {
  if (err instanceof SpeechEngineError) return err.fatal ? 'fatal' : 'transient'
  const text = (err instanceof Error ? err.message : typeof err === 'string' ? err : JSON.stringify(err) ?? '').toLowerCase()

  const status = /(?:\(|api error |http |status[: ]+|返回 ?|code[: ]+|error[: ]+)(\d{3})\b/.exec(text)?.[1]
  if (status) {
    const n = Number(status)
    if (n === 408 || n === 425 || n === 429 || (n >= 500 && n <= 599)) {
      // A 5xx that says the balance or plan is spent is not worth repeating.
      if (!/quota|balance|insufficient|余额|额度|欠费|\(2056\)/.test(text)) return 'transient'
    } else if (n >= 400) {
      return 'fatal'
    }
  }
  // MiniMax's throttle codes (1002 1041 2045 2062 2064) all mean "busy, try again".
  if (/\((?:1002|1041|2045|2062|2064)\)/.test(text)) return 'transient'
  // So do its server-side faults, which the Rust adapter (`minimax::code_class`) also
  // classes as transient: 1000 unknown, 1001 timeout, 1013 / 1024 internal, 1033 system
  // error. A repeat usually clears them. A message that also says the balance or plan
  // is spent is not worth repeating, as with the 5xx guard above.
  if (/\((?:1000|1001|1013|1024|1033)\)/.test(text) && !/quota|balance|insufficient|余额|额度|欠费|\(2056\)/.test(text)) {
    return 'transient'
  }
  // Rate limiting says "slow down", which a pause fixes — checked before the
  // generic "limit" wording below, which means a hard cap.
  if (/rate.?limit|too many requests|throttl|限流|频繁|请求过多|稍后再试/.test(text)) return 'transient'
  if (
    /unauthori[sz]ed|forbidden|api.?key|密钥|鉴权|认证|令牌|余额|欠费|额度|quota|insufficient|balance|payment|billing|exceed|超过|上限|too long|过长|limit|不支持|unsupported|not support|invalid|无效|参数|not found|不存在|permission|权限|敏感|违规|审核|moderation|没有配置|找不到/
      .test(text)
  ) return 'fatal'
  if (
    /time.?out|timed out|超时|network|网络|connection|连接|dns|reset|refused|temporar|暂时|unavailable|overload|过载|繁忙|busy|error sending request|empty audio|没有音频|空音频|interrupted|truncated|截断|不完整|请重试/
      .test(text)
  ) return 'transient'
  return 'fatal'
}

function messageOf(err: unknown): string {
  if (err instanceof Error) return err.message || String(err)
  if (typeof err === 'string') return err
  try { return JSON.stringify(err) ?? String(err) } catch { return String(err) }
}

// ── Autoplay unlock ───────────────────────────────────────────────────────────

/**
 * A 50 ms, 8 kHz mono silent WAV, built once at load.
 *
 * `unlock()` plays it from inside the click handler. The real audio arrives
 * seconds later, after a network round trip, long after the gesture is gone; a
 * WebView with a strict autoplay policy refuses a `play()` made then, and
 * playing *something* on this same element during the click is the standard way
 * to mark it user-started.
 *
 * It is insurance, not the whole story. Measured in a real WKWebView (a
 * current macOS, October 2026):
 * - with the policy Tauri's wry sets (`autoplay: true`, no user action needed)
 *   even an un-gestured `play()` works;
 * - with the strictest policy (`.all`) a delayed `play()` after a click ALSO
 *   worked without this clip, because WebKit's user activation is sticky;
 * - with no activation at all, `play()` was rejected with NotAllowedError, which
 *   `playError` turns into the visible `autoplay` error.
 * Older WebKits, which tracked the gesture per element, are what this is for.
 */
function buildSilentWav(): string {
  const n = 400
  const bytes = new Uint8Array(44 + n).fill(0x80) // 8-bit PCM: 0x80 is silence
  const dv = new DataView(bytes.buffer)
  const tag = (at: number, s: string) => { for (let i = 0; i < s.length; i++) bytes[at + i] = s.charCodeAt(i) }
  tag(0, 'RIFF'); dv.setUint32(4, 36 + n, true); tag(8, 'WAVE'); tag(12, 'fmt ')
  dv.setUint32(16, 16, true); dv.setUint16(20, 1, true); dv.setUint16(22, 1, true)
  dv.setUint32(24, 8000, true); dv.setUint32(28, 8000, true); dv.setUint16(32, 1, true); dv.setUint16(34, 8, true)
  tag(36, 'data'); dv.setUint32(40, n, true)
  let bin = ''
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i])
  return `data:audio/wav;base64,${btoa(bin)}`
}
export const SILENT_WAV = buildSilentWav()

// ── Engine ────────────────────────────────────────────────────────────────────

interface Flight {
  promise: Promise<SynthResult>
  signal: SynthSignal
  /** Runs that still want this result. At none, the request is flagged cancelled. */
  refs: Set<Run>
}

interface Run {
  chunks: string[]
  scope: string
  dead: boolean
  held: Set<Flight>
}

interface Slot {
  promise: Promise<SynthResult>
  /** Already resolved when it was asked for (a cache hit) or since. */
  done: boolean
}

export class SpeechEngine {
  private deps: SpeechEngineDeps
  private audio: AudioLike | null = null
  private cache = new Map<string, SynthResult>()
  private inflight = new Map<string, Flight>()
  private run: Run | null = null
  private current: SpeechState = 'idle'
  private cancelPlay: (() => void) | null = null
  /** Bumped by every pause(): a play() that began before it was cut short by the engine itself. */
  private pauseCount = 0
  private maxCache: number
  private maxCacheBytes: number
  private cacheBytes = 0
  private retryDelay: number
  private wait: (ms: number) => Promise<void>

  constructor(deps: SpeechEngineDeps) {
    this.deps = deps
    this.maxCache = Math.max(1, deps.maxCacheEntries ?? 20)
    this.maxCacheBytes = Math.max(1, deps.maxCacheBytes ?? 48_000_000)
    this.retryDelay = deps.retryDelayMs ?? 1500
    this.wait = deps.wait ?? ((ms) => new Promise((res) => setTimeout(res, ms)))
  }

  get state(): SpeechState { return this.current }
  /** True from the click until the read ends, errors or is stopped. */
  get isReading(): boolean { return this.current === 'loading' || this.current === 'playing' || this.current === 'paused' }
  get cacheSize(): number { return this.cache.size }
  clearCache(): void { this.cache.clear(); this.cacheBytes = 0 }

  /**
   * Call this SYNCHRONOUSLY from the click handler, before anything is awaited.
   *
   * Starts a silent clip on the persistent element (see `SILENT_WAV`), so a
   * WebView that wants a user gesture for sound sees the element user-started
   * before the real `play()` seconds later. A rejection here is expected outside
   * a gesture and is swallowed — if playback really is blocked, the real `play()`
   * reports it as a visible `autoplay` error.
   *
   * A no-op while a read is in progress: the element is then already playing (or
   * paused mid-clip) and was unlocked by the click that started it.
   */
  unlock(): void {
    if (this.isReading) return
    try {
      const audio = this.ensureAudio()
      audio.src = SILENT_WAV
      const p = audio.play()
      if (p && typeof (p as Promise<void>).catch === 'function') (p as Promise<void>).catch(() => {})
    } catch { /* unlocking is best effort */ }
  }

  /**
   * Start reading `chunks`. Replaces any read in progress. `scope` identifies the
   * provider, model and options (see `speechScope`) so cached audio is only reused
   * for the same voice.
   */
  read(chunks: string[], scope: string): void {
    this.invalidate()
    const items = chunks.map((c) => c.trim()).filter(Boolean)
    if (items.length === 0) {
      this.emit('idle', { index: 0, total: 0, reason: 'finished' })
      return
    }
    const run: Run = { chunks: items, scope, dead: false, held: new Set() }
    this.run = run
    this.emit('loading', { index: 0, total: items.length })
    // `drive` reports its own failures as an error state; this catch is the last
    // line of defence so nothing can become an unhandled rejection.
    this.drive(run).catch((err) => this.fail(run, 0, err))
  }

  pause(): boolean {
    const run = this.run
    if (!run || this.current !== 'playing') return false
    this.pauseCount++
    try { this.audio?.pause() } catch { /* a stuck pause still reads as paused */ }
    this.emit('paused', this.progress(run))
    return true
  }

  resume(): boolean {
    const run = this.run
    if (!run || this.current !== 'paused' || !this.audio) return false
    this.emit('playing', this.progress(run))
    const pausesAtStart = this.pauseCount
    try {
      const p = this.audio.play()
      if (p && typeof (p as Promise<void>).then === 'function') {
        ;(p as Promise<void>).then(undefined, (err) => {
          if (run.dead || this.interruptedByPause(err, pausesAtStart)) return
          this.fail(run, this.lastIndex, this.playError(err))
        })
      }
    } catch (err) {
      this.fail(run, this.lastIndex, this.playError(err))
    }
    return true
  }

  /** Stop everything and go idle. Safe to call in any state, any number of times. */
  stop(): void {
    this.invalidate()
    if (this.current !== 'idle') this.emit('idle', { index: 0, total: 0, reason: 'stopped' })
  }

  dispose(): void {
    this.stop()
    this.audio = null
    this.clearCache()
  }

  // ── internals ──────────────────────────────────────────────────────────────

  private lastIndex = 0

  private ensureAudio(): AudioLike {
    if (!this.audio) this.audio = this.deps.createAudio()
    return this.audio
  }

  private emit(state: SpeechState, info: SpeechInfo): void {
    this.current = state
    try { this.deps.onState(state, info) } catch { /* a throwing listener must not break playback */ }
  }

  private progress(run: Run): SpeechInfo {
    return { index: this.lastIndex, total: run.chunks.length }
  }

  /** Kill the current run: nothing it has in flight may start playing, and the audio goes quiet. */
  private invalidate(): void {
    const run = this.run
    if (!run) return
    run.dead = true
    this.run = null
    for (const f of run.held) {
      f.refs.delete(run)
      if (f.refs.size === 0) f.signal.cancelled = true
    }
    run.held.clear()
    const abort = this.cancelPlay
    this.cancelPlay = null
    abort?.()
    this.silence()
  }

  /** Stop sound and release the clip's memory, without leaving an error event behind. */
  private silence(): void {
    const audio = this.audio
    if (!audio) return
    try { audio.pause() } catch { /* ignore */ }
    try {
      if (audio.removeAttribute) audio.removeAttribute('src')
      else audio.src = ''
      audio.load?.()
    } catch { /* ignore */ }
  }

  private async drive(run: Run): Promise<void> {
    const total = run.chunks.length
    let slot: Slot | null = this.fetchClip(run, 0)
    for (let i = 0; i < total; i++) {
      if (run.dead || !slot) return
      this.lastIndex = i
      // Only say "loading" when there is actually something to wait for: between
      // two chunks the next one is normally already here, and flashing the
      // spinner every ten seconds would be noise.
      if (!slot.done && this.current !== 'loading') this.emit('loading', { index: i, total })
      let clip: SynthResult
      try {
        clip = await slot.promise
      } catch (err) {
        if (!run.dead) this.fail(run, i, err)
        return
      }
      if (run.dead) return
      // One ahead, started before this chunk plays so the two overlap.
      slot = i + 1 < total ? this.fetchClip(run, i + 1) : null
      try {
        await this.playClip(run, clip, i)
      } catch (err) {
        if (!run.dead) this.fail(run, i, err)
        return
      }
      if (run.dead) return
    }
    if (run.dead) return
    this.run = null
    for (const f of run.held) f.refs.delete(run)
    run.held.clear()
    this.silence()
    this.emit('idle', { index: total - 1, total, reason: 'finished' })
  }

  private fail(run: Run, index: number, err: unknown): void {
    if (run.dead) return
    const e = err instanceof SpeechEngineError
      ? err
      : new SpeechEngineError('synth', messageOf(err), classifySpeechError(err) === 'fatal')
    this.invalidate()
    this.emit('error', {
      index,
      total: run.chunks.length,
      error: { reason: e.reason, message: e.message, fatal: e.fatal },
    })
  }

  /**
   * A `play()` that is still pending when `pause()` is called rejects with
   * `AbortError` (HTML spec, "pause the media element"). That is this engine
   * interrupting itself, not a playback failure: the clip is still loaded, and the
   * resume that follows plays it from where it stopped. Reported as an error it
   * would turn a quick pause (a double click, or one that lands as the next chunk
   * starts) into a fatal read.
   */
  private interruptedByPause(err: unknown, pausesAtStart: number): boolean {
    const name = (err as { name?: string } | null)?.name
    return name === 'AbortError' && (this.current === 'paused' || this.pauseCount !== pausesAtStart)
  }

  private playError(err: unknown): SpeechEngineError {
    const name = (err as { name?: string } | null)?.name
    if (name === 'NotAllowedError') {
      return new SpeechEngineError('autoplay', messageOf(err), true)
    }
    if (name === 'NotSupportedError') {
      return new SpeechEngineError('decode', messageOf(err), true)
    }
    return new SpeechEngineError('playback', messageOf(err), true)
  }

  /** Resolve with audio for chunk `index`: cache, an in-flight request, or a new one. */
  private fetchClip(run: Run, index: number): Slot {
    const text = run.chunks[index]
    const key = speechCacheKey(run.scope, text)
    const hit = this.cacheGet(key)
    if (hit) return { promise: Promise.resolve(hit), done: true }

    let flight = this.inflight.get(key)
    if (flight) {
      // Someone (a read the user just replaced) already asked for this text. Take
      // over rather than pay for it twice — and un-cancel it, since it is wanted again.
      flight.signal.cancelled = false
    } else {
      flight = this.startFlight(key, text, run.scope)
    }
    flight.refs.add(run)
    run.held.add(flight)
    const slot: Slot = { promise: flight.promise, done: false }
    flight.promise.then(() => { slot.done = true }, () => { slot.done = true })
    return slot
  }

  private startFlight(key: string, text: string, scope: string): Flight {
    const signal: SynthSignal = { cancelled: false }
    const flight: Flight = { signal, refs: new Set(), promise: Promise.resolve({ dataUrl: '', mime: '' }) }
    flight.promise = (async () => {
      try {
        return await this.synthesizeWithRetry(key, text, scope, signal)
      } finally {
        if (this.inflight.get(key) === flight) this.inflight.delete(key)
      }
    })()
    // Consumers await the promise themselves; this stops the rejection counting as
    // unhandled in the window before (or if never) anyone does.
    flight.promise.catch(() => {})
    this.inflight.set(key, flight)
    return flight
  }

  private async synthesizeWithRetry(key: string, text: string, scope: string, signal: SynthSignal): Promise<SynthResult> {
    let retried = false
    for (;;) {
      try {
        const out = await this.deps.synthesize(text, signal, scope)
        if (!out || typeof out.dataUrl !== 'string' || !out.dataUrl) {
          throw new SpeechEngineError('synth', 'empty audio returned', false)
        }
        this.cacheSet(key, out)
        return out
      } catch (err) {
        const fatal = classifySpeechError(err) === 'fatal'
        if (!fatal && !retried && !signal.cancelled) {
          retried = true
          await this.wait(this.retryDelay)
          if (!signal.cancelled) continue
        }
        throw err instanceof SpeechEngineError ? err : new SpeechEngineError('synth', messageOf(err), fatal)
      }
    }
  }

  private cacheGet(key: string): SynthResult | undefined {
    const v = this.cache.get(key)
    if (v) {
      // Most recently used goes last; the first key is the one to evict.
      this.cache.delete(key)
      this.cache.set(key, v)
    }
    return v
  }

  private cacheSet(key: string, value: SynthResult): void {
    const prior = this.cache.get(key)
    if (prior) this.cacheBytes -= prior.dataUrl.length
    this.cache.delete(key)
    this.cache.set(key, value)
    this.cacheBytes += value.dataUrl.length
    // Evict the least recently used until both ceilings hold — but never the entry
    // just stored: one oversized clip is still worth replaying.
    while (this.cache.size > 1 && (this.cache.size > this.maxCache || this.cacheBytes > this.maxCacheBytes)) {
      const oldest = this.cache.keys().next().value
      if (oldest === undefined) break
      const gone = this.cache.get(oldest)
      this.cache.delete(oldest)
      if (gone) this.cacheBytes -= gone.dataUrl.length
    }
  }

  /** Play one clip on the persistent element; resolves when it ends, rejects on failure. */
  private playClip(run: Run, clip: SynthResult, index: number): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const audio = this.ensureAudio()
      let settled = false
      const cleanup = () => {
        audio.removeEventListener('ended', onEnded)
        audio.removeEventListener('error', onError)
        if (this.cancelPlay === abort) this.cancelPlay = null
      }
      const settle = (done: () => void) => {
        if (settled) return
        settled = true
        cleanup()
        done()
      }
      const onEnded = () => settle(resolve)
      const onError = () => settle(() => reject(new SpeechEngineError(
        'decode', `audio could not be decoded (${clip.mime || 'unknown type'})`, true,
      )))
      // The run was stopped or replaced: let the loop unwind quietly.
      const abort = () => settle(resolve)
      this.cancelPlay = abort
      audio.addEventListener('ended', onEnded)
      audio.addEventListener('error', onError)
      try {
        audio.src = clip.dataUrl
        const pausesAtStart = this.pauseCount
        const started = audio.play()
        Promise.resolve(started).then(
          () => {
            // Not "playing" if the clip already ended, the run died, or the user paused first.
            if (!settled && !run.dead && this.current !== 'paused') {
              this.emit('playing', { index, total: run.chunks.length })
            }
          },
          (err) => {
            if (settled || run.dead) return
            // Cut short by pause(): the clip stays loaded, resume() restarts it and
            // `ended` settles this promise as usual.
            if (this.interruptedByPause(err, pausesAtStart)) return
            settle(() => reject(this.playError(err)))
          },
        )
      } catch (err) {
        settle(() => reject(this.playError(err)))
      }
    })
  }
}
