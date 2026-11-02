/**
 * Usage aggregation for the canvas footer.
 *
 * Split from `items-reducer.ts` to keep the reducer under the web hard-rule
 * line bound; the fold contract lives here: authoritative `usage` events are
 * summed per thread, and the HTTP seed only ever fills an empty channel.
 */
import type { Usage, ThreadUsageResponse } from '../../../shared/contracts'
import type { ItemsSnapshot, UsageAggregate } from './types'

/** Fold one authoritative `usage` sample into the per-thread aggregate. */
export function foldUsage(current: UsageAggregate | null, sample: Usage): UsageAggregate {
  return {
    samples: (current?.samples ?? 0) + 1,
    promptTokens: (current?.promptTokens ?? 0) + sample.prompt_tokens,
    completionTokens: (current?.completionTokens ?? 0) + sample.completion_tokens,
    totalTokens: (current?.totalTokens ?? 0) + sample.total_tokens,
    // A provider that omits cost keeps the aggregate costless; mixing a costed
    // sample into a costless aggregate would present a partial sum as total.
    costUsd: current?.costUsd != null && sample.cost_usd != null ? current.costUsd + sample.cost_usd : null,
    lastPromptTokens: sample.prompt_tokens
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
      lastPromptTokens: null
    }
  }
}
