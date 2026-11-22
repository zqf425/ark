/**
 * Usage aggregation for the canvas footer.
 *
 * Split from `items-reducer.ts` to keep the reducer under the web hard-rule
 * line bound; the fold contract lives here: authoritative `usage` events are
 * summed per thread, and the HTTP seed only ever fills an empty channel.
 */
import type { Usage, ThreadUsageResponse } from '../../../shared/contracts'
import type { ItemsSnapshot, UsageAggregate } from './types'

/**
 * Fold one authoritative `usage` sample into the per-thread aggregate.
 *
 * `tokensPerSec` is the speed the reducer measured for the round this sample
 * closes; passing null (no measurable streaming window) keeps no stale value.
 */
export function foldUsage(
  current: UsageAggregate | null,
  sample: Usage,
  tokensPerSec: number | null = null
): UsageAggregate {
  return {
    samples: (current?.samples ?? 0) + 1,
    promptTokens: (current?.promptTokens ?? 0) + sample.prompt_tokens,
    completionTokens: (current?.completionTokens ?? 0) + sample.completion_tokens,
    totalTokens: (current?.totalTokens ?? 0) + sample.total_tokens,
    // Cost sums whenever any side has it, and is null only when no sample ever
    // carried one — the same `sum_opt_f` semantics as the Rust aggregate, so
    // the footer never hides a cost the server would report.
    costUsd: sample.cost_usd != null ? (current?.costUsd ?? 0) + sample.cost_usd : current?.costUsd ?? null,
    // Cache counters fold with the same `sum_opt` semantics: a sample without
    // the counter contributes nothing, and the footer only derives a rate from
    // the hit/miss pair — never from `hit / prompt_tokens`.
    cacheHitTokens:
      sample.cache_hit_tokens != null
        ? (current?.cacheHitTokens ?? 0) + sample.cache_hit_tokens
        : current?.cacheHitTokens ?? null,
    cacheMissTokens:
      sample.cache_miss_tokens != null
        ? (current?.cacheMissTokens ?? 0) + sample.cache_miss_tokens
        : current?.cacheMissTokens ?? null,
    // Same `sum_opt` rule: a sample without a measured duration contributes
    // nothing, and the total stays null until at least one was timed.
    durationMs: sample.duration_ms != null ? (current?.durationMs ?? 0) + sample.duration_ms : current?.durationMs ?? null,
    // The active span is server-owned (it freezes on user gates): live events
    // keep the last HTTP-seeded value and the next refresh corrects it.
    activeMs: current?.activeMs ?? null,
    lastPromptTokens: sample.prompt_tokens,
    lastRoundTokensPerSec: tokensPerSec
  }
}

/**
 * Seed the usage aggregate from `GET /v1/threads/{id}/usage` after the detail
 * load. Only fills an empty channel: if a live `usage` event already landed
 * (SSE can win the race against the HTTP seed), overwriting it would double
 * count that sample.
 */
export function seedUsage(state: ItemsSnapshot, response: ThreadUsageResponse): ItemsSnapshot {
  if (state.usage) return state
  return {
    ...state,
    usage: {
      samples: response.samples,
      promptTokens: response.prompt_tokens,
      completionTokens: response.completion_tokens,
      totalTokens: response.total_tokens,
      costUsd: response.cost_usd ?? null,
      cacheHitTokens: response.cache_hit_tokens ?? null,
      cacheMissTokens: response.cache_miss_tokens ?? null,
      // The seed IS the lifetime spanning total: it covers every round in the
      // log, including rounds of turns the client never rendered.
      durationMs: response.duration_ms ?? null,
      // Same for the active span: server-folded wall clock with user-gate
      // waits already excluded, covering turns the client never rendered.
      activeMs: response.active_ms ?? null,
      // It carries no per-round timing though: prompt fill and speed describe
      // the newest round only, and the seed knows no "newest".
      lastPromptTokens: null,
      lastRoundTokensPerSec: null
    }
  }
}
