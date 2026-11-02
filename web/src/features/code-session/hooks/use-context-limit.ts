import { useEffect, useState } from 'react'
import { getRuntimeConfig } from '../../settings/api/settings-api'

/** How long a fetched limit may serve as the meter's denominator. */
const CACHE_TTL_MS = 60_000

/** Module-level cache with a TTL: the tuning limit can change in settings. */
let cache: { limit: number; at: number } | null = null

/**
 * Context window limit (`tuning.context_limit_tokens`) for the usage meter.
 * Best effort: an unreadable config leaves the meter without a percentage
 * instead of blocking the footer, and a cached value is refreshed after the
 * TTL so a settings change cannot go unnoticed for longer than a minute.
 */
export function useContextLimit(): number | null {
  const [limit, setLimit] = useState<number | null>(cache?.limit ?? null)

  useEffect(() => {
    if (cache && Date.now() - cache.at < CACHE_TTL_MS) return
    const controller = new AbortController()
    void getRuntimeConfig(controller.signal)
      .then((config) => {
        cache = { limit: config.tuning.context_limit_tokens, at: Date.now() }
        setLimit(cache.limit)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [])

  return limit
}
