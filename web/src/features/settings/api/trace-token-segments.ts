import type { Usage } from '../../../shared/contracts'

/**
 * Token segment shape shared by the trace section's session overview and the
 * single-turn timeline details.
 *
 * Cache hits are a subset of `prompt_tokens`, and `cache_miss_tokens` (when the
 * provider reports it) counts only the cache lookups that missed. The four
 * disjoint parts are non-cached input, cache miss, cache hit and output; they
 * sum to `total = prompt + completion`, so every percentage uses one honest
 * denominator. Missing counters are never invented: a provider that reports
 * no cache fields yields only non-cached input + output, one that reports a
 * hit but no miss omits the miss segment. Presentation arithmetic over wire
 * values only.
 */
export type TraceTokenSegment = {
  key: 'noncached' | 'miss' | 'hit' | 'output'
  labelKey:
    | 'settings.trace.gantt.tokenNoncached'
    | 'settings.trace.gantt.tokenMiss'
    | 'settings.trace.gantt.tokenCached'
    | 'settings.trace.gantt.tokenOutput'
  value: number
}

/**
 * Front-end cache hit rate, in the same units as the session header:
 * `hit / (hit + miss)`. A missing miss counter makes the rate unknown — it is
 * never reconstructed from `prompt_tokens`, which also counts non-cacheable
 * inputs. The wire `cache_hit_rate` is deliberately ignored here: a sampled
 * aggregate can omit it even when both counters are present.
 * `cache_hit_rate_approximate` (from the wire) marks a derived miss counter.
 */
export function traceCacheHitRate(usage: Usage | undefined): number | null {
  const hit = usage?.cache_hit_tokens
  const miss = usage?.cache_miss_tokens
  if (hit == null || miss == null || hit + miss <= 0) return null
  return hit / (hit + miss)
}

/** Cache fields are provider optional; all three absent means "not reported". */
export function traceTokenCacheReported(usage: Usage | undefined): boolean {
  return usage?.cache_hit_tokens != null || usage?.cache_miss_tokens != null || usage?.cache_hit_rate != null
}

/**
 * Split prompt into up to four mutually exclusive parts. `hit` and `miss` are
 * clipped to `prompt` so overlapping or oversized provider counters cannot
 * double-count; the remainder is non-cached input. When `miss` is absent the
 * cache lookup share is unknown and no miss segment is emitted.
 */
export function traceTokenSegments(usage: Usage | undefined): TraceTokenSegment[] {
  const prompt = Math.max(0, usage?.prompt_tokens ?? 0)
  const completion = Math.max(0, usage?.completion_tokens ?? 0)
  const reported = traceTokenCacheReported(usage)
  const rawHit = reported ? Math.max(0, usage?.cache_hit_tokens ?? 0) : 0
  const hit = Math.min(rawHit, prompt)
  const miss = usage?.cache_miss_tokens == null
    ? 0
    : Math.min(Math.max(0, usage.cache_miss_tokens), Math.max(0, prompt - hit))
  const noncached = Math.max(0, prompt - hit - miss)
  const segments: TraceTokenSegment[] = [
    { key: 'noncached', labelKey: 'settings.trace.gantt.tokenNoncached', value: noncached },
    { key: 'miss', labelKey: 'settings.trace.gantt.tokenMiss', value: miss },
    { key: 'hit', labelKey: 'settings.trace.gantt.tokenCached', value: hit },
    { key: 'output', labelKey: 'settings.trace.gantt.tokenOutput', value: completion }
  ]
  return segments.filter((segment) => segment.value > 0)
}
