import type { Usage } from '../../../shared/contracts'

/**
 * Token segment shape shared by the trace section's session overview and the
 * single-turn timeline details.
 *
 * `cached` is a subset of `prompt` (OpenAI counts cache hits inside
 * prompt_tokens; Anthropic decomposes prompt into input + cache_read +
 * cache_creation), so input-total and cache-hit must never be shown side by
 * side as if disjoint. The three disjoint parts sum to `total`, making every
 * percentage honest. Only presentation arithmetic over wire values; no
 * provider value is invented when the cache counters are absent.
 */
export type TraceTokenSegment = {
  key: 'uncached' | 'hit' | 'output'
  labelKey: 'settings.trace.gantt.tokenUncached' | 'settings.trace.gantt.tokenCached' | 'settings.trace.gantt.tokenOutput'
  value: number
}

/** Cache fields are provider optional; all three absent means "not reported". */
export function traceTokenCacheReported(usage: Usage | undefined): boolean {
  return usage?.cache_hit_tokens != null || usage?.cache_miss_tokens != null || usage?.cache_hit_rate != null
}

/** The mutually exclusive input-miss / cache-hit / output segments. */
export function traceTokenSegments(usage: Usage | undefined): TraceTokenSegment[] {
  const prompt = Math.max(0, usage?.prompt_tokens ?? 0)
  const completion = Math.max(0, usage?.completion_tokens ?? 0)
  const cached = traceTokenCacheReported(usage)
    ? Math.max(0, Math.min(usage?.cache_hit_tokens ?? 0, prompt))
    : 0
  const uncached = Math.max(0, prompt - cached)
  const segments: TraceTokenSegment[] = [
    { key: 'uncached', labelKey: 'settings.trace.gantt.tokenUncached', value: uncached },
    { key: 'hit', labelKey: 'settings.trace.gantt.tokenCached', value: cached },
    { key: 'output', labelKey: 'settings.trace.gantt.tokenOutput', value: completion }
  ]
  return segments.filter((segment) => segment.value > 0)
}
