/**
 * Usage aggregation for the canvas footer. One store-owned request ticket guards
 * each HTTP snapshot; only samples received while that ticket is in flight are
 * retained for the coverage replay.
 */
import type { ThreadId, Usage, ThreadUsageResponse } from '../../../shared/contracts'
import type { ItemsSnapshot, UsageAggregate, UsageRequest, UsageTailNode, UsageTailSample } from './types'

const sumOptional = (left: number | null, right: number | null): number | null =>
  left == null ? right : right == null ? left : left + right

/** Fold one authoritative sample with Rust `Usage::merge` incompleteness. */
export function foldUsage(
  current: UsageAggregate | null,
  sample: Usage,
  tokensPerSec: number | null = null
): UsageAggregate {
  const cacheCountersIncomplete =
    (current?.cacheCountersIncomplete ?? false) ||
    sample.cache_hit_tokens == null ||
    sample.cache_miss_tokens == null
  const durationIncomplete =
    (current?.durationIncomplete ?? false) ||
    (current !== null && current.samples > 0 && sample.duration_ms == null) ||
    sample.duration_ms == null
  return {
    samples: (current?.samples ?? 0) + 1,
    promptTokens: (current?.promptTokens ?? 0) + sample.prompt_tokens,
    completionTokens: (current?.completionTokens ?? 0) + sample.completion_tokens,
    totalTokens: (current?.totalTokens ?? 0) + sample.total_tokens,
    costUsd: sumOptional(current?.costUsd ?? null, sample.cost_usd ?? null),
    cacheHitTokens: sumOptional(current?.cacheHitTokens ?? null, sample.cache_hit_tokens ?? null),
    cacheMissTokens: sumOptional(current?.cacheMissTokens ?? null, sample.cache_miss_tokens ?? null),
    cacheCountersIncomplete,
    durationIncomplete,
    durationMs: durationIncomplete
      ? null
      : sumOptional(current?.durationMs ?? null, sample.duration_ms ?? null),
    activeMs: current?.activeMs ?? null,
    lastPromptTokens: sample.prompt_tokens > 0 ? sample.prompt_tokens : current?.lastPromptTokens ?? null,
    lastRoundTokensPerSec: tokensPerSec
  }
}

function usageFromResponse(response: ThreadUsageResponse): UsageAggregate {
  const samples = response.samples
  const cacheCountersIncomplete =
    samples > 0 && (response.cache_hit_tokens == null || response.cache_miss_tokens == null)
  const durationIncomplete = samples > 0 && response.duration_ms == null
  return {
    samples,
    promptTokens: response.prompt_tokens,
    completionTokens: response.completion_tokens,
    totalTokens: response.total_tokens,
    costUsd: response.cost_usd ?? null,
    cacheHitTokens: response.cache_hit_tokens ?? null,
    cacheMissTokens: response.cache_miss_tokens ?? null,
    cacheCountersIncomplete,
    durationIncomplete,
    durationMs: durationIncomplete ? null : response.duration_ms ?? null,
    activeMs: response.active_ms ?? null,
    lastPromptTokens: response.last_valid_prompt_tokens ?? null,
    lastRoundTokensPerSec: null
  }
}

let nextUsageGeneration = 0

/** Capture the store snapshot that an HTTP usage request will own. */
export function createUsageRequest(state: ItemsSnapshot, owner: ThreadId): UsageRequest {
  nextUsageGeneration += 1
  return {
    owner,
    seq: state.lastSeq,
    generation: nextUsageGeneration,
    baseSamples: state.usage?.samples ?? 0
  }
}

/** True only for the newest request belonging to this thread projection. */
export function ownsUsageRequest(state: ItemsSnapshot, request: UsageRequest): boolean {
  return state.usageRequest?.owner === request.owner &&
    state.usageRequest.generation === request.generation
}

/** Duplicate detection for the rare usage frame that arrives below `lastSeq`. */
export function usageTailHasSeq(tail: UsageTailNode | null, seq: number): boolean {
  for (let node = tail; node; node = node.next) {
    if (node.sample.seq === seq) return true
  }
  return false
}

/** O(1) persistent append; previously committed nodes stay immutable. */
export function appendUsageTail(tail: UsageTailNode | null, sample: UsageTailSample): UsageTailNode {
  return { sample, next: tail }
}

/** Materialize only when a snapshot settles or a test inspects the tail. */
export function usageTailSamples(tail: UsageTailNode | null): UsageTailSample[] {
  const samples: UsageTailSample[] = []
  for (let node = tail; node; node = node.next) samples.push(node.sample)
  return samples.reverse()
}

/** Response coverage is a sample-count prefix, so replay must follow event order. */
export function replayableUsageTail(tail: UsageTailNode | null, afterSeq: number): UsageTailSample[] {
  return usageTailSamples(tail)
    .filter((sample) => sample.seq > afterSeq)
    .sort((left, right) => left.seq - right.seq)
}

/**
 * The response watermark identifies exactly which retained SSE samples the
 * aggregate already contains. Older servers fall back to sample-count coverage.
 * Replay only the suffix, then release both the owner and its bounded tail.
 */
export function seedUsage(
  state: ItemsSnapshot,
  response: ThreadUsageResponse,
  request: UsageRequest
): ItemsSnapshot {
  if (!ownsUsageRequest(state, request)) return state
  if (
    response.thread_id !== request.owner ||
    request.seq < state.usageSeedSeq ||
    response.samples < request.baseSamples ||
    (response.last_seq != null && response.last_seq < request.seq)
  ) return cancelUsageRequest(state, request)
  const tail = replayableUsageTail(state.usageTail, request.seq)
  const coveredSamples = response.samples - request.baseSamples
  const responseSeq = response.last_seq
  const remaining = responseSeq == null
    ? tail.slice(Math.min(coveredSamples, tail.length))
    : tail.filter((sample) => sample.seq > responseSeq)
  let usage = usageFromResponse(response)
  for (const sample of remaining) {
    usage = foldUsage(usage, sample.usage, sample.roundTokensPerSec)
  }
  if (usage.activeMs !== null && state.usage?.activeMs != null) {
    usage = { ...usage, activeMs: Math.max(usage.activeMs, state.usage.activeMs) }
  }
  return {
    ...state,
    usage,
    usageTurns: Math.max(state.usageTurns ?? 0, response.turns),
    usageRequest: null,
    usageTail: null,
    usageSeedSeq: Math.max(state.usageSeedSeq, request.seq)
  }
}

/** Release a failed/aborted request without letting it clear a newer owner. */
export function cancelUsageRequest(state: ItemsSnapshot, request: UsageRequest): ItemsSnapshot {
  if (!ownsUsageRequest(state, request)) return state
  return { ...state, usageRequest: null, usageTail: null }
}
