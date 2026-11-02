import { useEffect, useState } from 'react'
import { getRuntimeConfig } from '../../settings/api/settings-api'

/** Module-level cache: the tuning limit rarely changes within a page session. */
let cachedLimit: number | null = null

/**
 * Context window limit (`tuning.context_limit_tokens`) for the usage meter.
 * Best effort: an unreadable config leaves the meter without a percentage
 * instead of blocking the footer.
 */
export function useContextLimit(): number | null {
  const [limit, setLimit] = useState<number | null>(cachedLimit)

  useEffect(() => {
    if (cachedLimit != null) return
    const controller = new AbortController()
    void getRuntimeConfig(controller.signal)
      .then((config) => {
        cachedLimit = config.tuning.context_limit_tokens
        setLimit(cachedLimit)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [])

  return limit
}
