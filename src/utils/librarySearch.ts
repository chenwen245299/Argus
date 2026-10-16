/** Debounce input and serialize requests, retaining only the latest pending
 * query. A slow response cannot reopen a cleared search or another library. */
export function createLibrarySearch<T>(options: {
  root: () => string | null
  search: (query: string, root: string) => Promise<T[]>
  results: (query: string, hits: T[]) => void
  clear: () => void
  busy: (value: boolean) => void
  error: (query: string, error: unknown) => void
  delay?: number
}) {
  type Request = { query: string; root: string; revision: number }
  let revision = 0
  let timer: ReturnType<typeof setTimeout> | null = null
  let pending: Request | null = null
  let running = false

  function cancel() {
    revision++
    if (timer) clearTimeout(timer)
    timer = null
    pending = null
    options.busy(false)
  }

  async function drain() {
    if (running || !pending) return
    const request = pending
    pending = null
    running = true
    const current = () => revision === request.revision && options.root() === request.root
    try {
      const hits = await options.search(request.query, request.root)
      if (current()) options.results(request.query, hits)
    } catch (error) {
      if (current()) options.error(request.query, error)
    } finally {
      running = false
      if (current()) options.busy(false)
      void drain()
    }
  }

  function schedule(value: string, immediate = false) {
    cancel()
    const query = value.trim()
    const root = options.root()
    if (!query || !root) {
      options.clear()
      return
    }
    const request = { query, root, revision }
    options.busy(true)
    const ready = () => {
      timer = null
      pending = request
      void drain()
    }
    if (immediate) ready()
    else timer = setTimeout(ready, options.delay ?? 180)
  }

  function clear() {
    cancel()
    options.clear()
  }

  return { schedule, cancel, clear }
}
