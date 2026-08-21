import type { ServerToolTrace } from '../types'

/**
 * Fold one `-servertools` payload into what an answer has collected so far.
 *
 * The agent loop emits a trace per round, so an answer that took five rounds
 * reports five times. Citations and images are unioned (a page found in round
 * one must not vanish when round two reports its own), while call counts take
 * the larger figure, since each round's usage is cumulative rather than
 * incremental.
 */
export function mergeServerToolTrace(
  existing: ServerToolTrace | undefined | null,
  incoming: ServerToolTrace | undefined | null
): ServerToolTrace | undefined {
  if (!incoming) return existing ?? undefined
  const base: ServerToolTrace = existing ?? { citations: [], images: [], calls: {} }

  const citations = [...base.citations]
  for (const c of incoming.citations ?? []) {
    if (!citations.some(seen => seen.url === c.url)) citations.push(c)
  }

  const images = [...base.images]
  for (const src of incoming.images ?? []) {
    if (!images.includes(src)) images.push(src)
  }

  const calls: Record<string, number> = { ...base.calls }
  for (const [tool, n] of Object.entries(incoming.calls ?? {})) {
    calls[tool] = Math.max(calls[tool] ?? 0, n)
  }

  return { citations, images, calls }
}

/**
 * The version of a trace worth writing to disk.
 *
 * Citations are the part with lasting value, so they stay. Images are kept only
 * when they are remote URLs: OpenRouter normally hands back a link, but a data
 * URI would put megabytes of base64 into a conversation file that is rewritten
 * on every token of the next answer.
 */
export function persistableServerToolTrace(
  trace: ServerToolTrace | undefined | null
): ServerToolTrace | undefined {
  if (!trace) return undefined
  return {
    citations: trace.citations ?? [],
    images: (trace.images ?? []).filter(src => !src.startsWith('data:')),
    calls: trace.calls ?? {},
  }
}
