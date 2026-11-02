import { useEffect, useState } from 'react'
import { getRuntimeConfig } from '../../settings/api/settings-api'

/** How long a fetched limit may serve as the meter's denominator. */
const CACHE_TTL_MS = 60_000

/** Module-level cache with a TTL: the tuning limit can change in settings. */
let cache: { limit: number; at: number } | null = null

/**
 * Context window limit (`tuning.context_limit_tokens`) for the usage meter.
 *
 * The footer stays mounted across thread switches, so a one-shot fetch could
 * serve a stale limit forever after the user edits tuning settings. The cached
 * value therefore expires after `CACHE_TTL_MS` and the hook re-fetches on a
 * matching interval; a failed refresh keeps the previous denominator.
 */
export function useContextLimit(): number | null {
  const [limit, setLimit] = useState<number | null>(cache?.limit ?? null)

  useEffect(() => {
    let cancelled = false
    const refresh = (): void => {
      if (cache && Date.now() - cache.at < CACHE_TTL_MS) return
      void getRuntimeConfig()
        .then((config) => {
          if (cancelled) return
          cache = { limit: config.tuning.context_limit_tokens, at: Date.now() }
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
