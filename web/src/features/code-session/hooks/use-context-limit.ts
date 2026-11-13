import { useEffect, useState } from 'react'
import { listProviders } from '../../settings/api/providers-api'

/** How long a fetched limit may serve as the meter's denominator. */
const CACHE_TTL_MS = 60_000

/** Module-level cache with a TTL: the provider table can change in settings. */
let cache: { limit: number | null; at: number } | null = null

/**
 * Compaction threshold for the usage meter, derived from the *effective*
 * provider's context window (80% of it, output reserved) and reported by
 * `GET /v1/runtime/providers` as `compaction_threshold_tokens`. A provider
 * that declares no window derives from the 256K default window, so the meter
 * matches what the runtime actually compacts against.
 *
 * `null` means "no provider resolves at all": the meter hides instead of
 * dividing by a guessed denominator.
 *
 * The footer stays mounted across thread switches, so a one-shot fetch could
 * serve a stale limit forever after the user edits providers. The cached value
 * therefore expires after `CACHE_TTL_MS` and the hook re-fetches on a matching
 * interval; a failed refresh keeps the previous denominator.
 */
export function useContextLimit(): number | null {
  const [limit, setLimit] = useState<number | null>(cache?.limit ?? null)

  useEffect(() => {
    let cancelled = false
    const refresh = (): void => {
      if (cache && Date.now() - cache.at < CACHE_TTL_MS) return
      void listProviders()
        .then((response) => {
          if (cancelled) return
          cache = { limit: response.compaction_threshold_tokens ?? null, at: Date.now() }
          setLimit(cache.limit)
        })
        .catch(() => {})
    }
    refresh()
    const timer = setInterval(refresh, CACHE_TTL_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [])

  return limit
}
