import { useEffect, useState } from 'react'
import { listProviders } from '../../settings/api/providers-api'

/** How long a fetched reading may serve as the meter's denominators. */
const CACHE_TTL_MS = 60_000

/** Module-level cache with a TTL: the provider table can change in settings. */
let cache: { reading: ContextReading | null; at: number } | null = null

/**
 * What the context meter displays, both served by `GET /v1/runtime/providers`
 * for the *effective* provider:
 *
 * - `window` (`context_window_tokens`): the context window the runtime
 *   compacts against — the provider's declared window, or the 256K default.
 *   The meter's denominator: "X / 500K" means X of the model's real window.
 * - `threshold` (`compaction_threshold_tokens`): the point the runtime folds
 *   the prefix at (80% of the usable window, output reserved). Drawn as a
 *   tick on the meter and used for the warning tone.
 *
 * Either is `null` only when no provider resolves at all: the meter hides
 * instead of dividing by a guessed denominator.
 *
 * The footer stays mounted across thread switches, so a one-shot fetch could
 * serve a stale reading forever after the user edits providers. The cached
 * value therefore expires after `CACHE_TTL_MS` and the hook re-fetches on a
 * matching interval; a failed refresh keeps the previous reading.
 */
export type ContextReading = {
  /** Declared-or-default context window, the meter's denominator. */
  window: number | null
  /** Derived compaction point, the warning line inside the window. */
  threshold: number | null
}

export function useContextLimit(): ContextReading {
  const [reading, setReading] = useState<ContextReading | null>(cache?.reading ?? null)

  useEffect(() => {
    let cancelled = false
    const refresh = (): void => {
      if (cache && Date.now() - cache.at < CACHE_TTL_MS) return
      void listProviders()
        .then((response) => {
          if (cancelled) return
          cache = {
            reading: {
              window: response.context_window_tokens ?? null,
              threshold: response.compaction_threshold_tokens ?? null
            },
            at: Date.now()
          }
          setReading(cache.reading)
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

  return reading ?? { window: null, threshold: null }
}
