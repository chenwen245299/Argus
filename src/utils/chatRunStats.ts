/** Conversation accounting is independent of the retained model context.
 * Each request replaces its own sample; regeneration starts another request. */
export interface ChatRun {
  id: string
  answerId: string
  providerId: string
  modelId: string
  kind?: 'compaction'
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  cacheHitTokens?: number
  modelCalls?: number
  modelDurationMs?: number
  durationMs?: number
  toolCalls?: number
}

export interface StreamUsagePayload {
  input_tokens?: number
  output_tokens?: number
  total_tokens?: number
  cache_hit_tokens?: number
  cache_reported?: boolean
  model_calls?: number
  model_duration_ms?: number
  cost_usd?: number | null
}

export interface ChatRunOwner { usageRuns?: Record<string, ChatRun> }

export function legacyChatRun(answer: {
  id: string; inputTokens?: number; outputTokens?: number; totalTokens?: number; cacheHitTokens?: number;
  startedAt?: number; endedAt?: number;
}, providerId: string, modelId: string, toolCalls = 0): ChatRun {
  return { id: `legacy:${answer.id}`, answerId: answer.id, providerId, modelId,
    inputTokens: answer.inputTokens, outputTokens: answer.outputTokens, totalTokens: answer.totalTokens,
    // Old adapters emitted zero even without a provider cache field.
    cacheHitTokens: answer.cacheHitTokens && answer.cacheHitTokens > 0 ? answer.cacheHitTokens : undefined,
    modelCalls: !toolCalls && count(answer.inputTokens) ? 1 : undefined, toolCalls,
    durationMs: count(answer.startedAt) && count(answer.endedAt) && answer.endedAt > answer.startedAt
      ? answer.endedAt - answer.startedAt : undefined }
}

export function recordCompactionUsage(owner: ChatRunOwner, usage: StreamUsagePayload & { id: string },
  providerId: string, modelId: string, legacy: ChatRun[] = [], cacheReported = false) {
  const id = `compaction:${usage.id}`
  const run = owner.usageRuns?.[id] ?? beginChatRun(owner, { id, answerId: id, providerId, modelId, kind: 'compaction' }, legacy)
  applyRunUsage(run, usage, cacheReported)
  return run
}

const count = (value: number | undefined): value is number =>
  typeof value === 'number' && Number.isFinite(value) && value >= 0

export function applyRunUsage(run: ChatRun, usage: StreamUsagePayload, legacyCacheReported = false) {
  if (count(usage.input_tokens)) run.inputTokens = usage.input_tokens
  if (count(usage.output_tokens)) run.outputTokens = usage.output_tokens
  if (count(usage.total_tokens)) run.totalTokens = usage.total_tokens
  if (count(usage.model_calls)) run.modelCalls = usage.model_calls
  if (count(usage.model_duration_ms)) run.modelDurationMs = usage.model_duration_ms
  run.cacheHitTokens = count(usage.cache_hit_tokens) && (usage.cache_reported === true
    || (usage.cache_reported === undefined && (legacyCacheReported || usage.cache_hit_tokens > 0)))
    ? usage.cache_hit_tokens : undefined
}

/** Seed old saved answers once. The ledger thereafter survives answer deletion,
 * branch changes and regeneration; active samples never add to themselves. */
export function beginChatRun(owner: ChatRunOwner, run: ChatRun, legacy: ChatRun[] = []): ChatRun {
  owner.usageRuns ??= Object.fromEntries(legacy.map(sample => [sample.id, sample]))
  owner.usageRuns[run.id] = run
  return owner.usageRuns[run.id]
}

export function summarizeChatRuns(runs: ChatRun[]) {
  let input = 0, output = 0, total = 0, calls = 0, tools = 0, measured = 0, callsEstimated = false
  let cacheInput = 0, cacheHits = 0, cacheComplete = true
  for (const run of runs) {
    if (count(run.inputTokens)) input += run.inputTokens
    if (count(run.outputTokens)) output += run.outputTokens
    if (count(run.totalTokens)) { total += run.totalTokens; measured++ }
    else if (count(run.inputTokens) && count(run.outputTokens)) { total += run.inputTokens + run.outputTokens; measured++ }
    // A legacy answer's unknown tool rounds must not be guessed from tool count.
    if (count(run.modelCalls)) calls += run.modelCalls
    else if (count(run.inputTokens)) { calls++; callsEstimated = true }
    if (count(run.toolCalls)) tools += run.toolCalls
    if (count(run.inputTokens) && run.inputTokens > 0) {
      if (count(run.cacheHitTokens)) {
        cacheInput += run.inputTokens
        cacheHits += Math.min(run.cacheHitTokens, run.inputTokens)
      } else cacheComplete = false
    }
  }
  const latest = [...runs].reverse().find(run => run.kind !== 'compaction' && count(run.outputTokens)
    && (run.modelDurationMs ?? run.durationMs ?? 0) > 0)
  const duration = latest?.modelDurationMs ?? latest?.durationMs
  const speed = latest && duration ? Math.round(latest.outputTokens! * 1000 / duration) : null
  return { input, output, total, calls, tools, measured, callsEstimated,
    cachePercent: cacheComplete && cacheInput > 0 ? Math.round(cacheHits * 100 / cacheInput) : null,
    speed: speed && speed > 0 ? speed : null }
}

export function compactTokenCount(value: number): string {
  if (value >= 1_000_000) return `${Number((value / 1_000_000).toFixed(1))}M`
  if (value >= 1_000) return `${Number((value / 1_000).toFixed(1))}k`
  return String(Math.round(value))
}
