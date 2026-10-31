/**
 * Runtime status polling: the unauthenticated liveness probe plus the runtime
 * info endpoint for version / data dir.
 *
 * Both calls live in `../api/runtime-api`, which owns the paths: a hook must not
 * carry a runtime route of its own (design §7.10.2).
 * Version and liveness come from the runtime, never from a build-time constant.
 */
import { useCallback, useEffect } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { getHealth, getRuntimeInfo } from '../api/runtime-api'
import { useCodeSessionStore } from '../state/store'

const POLL_INTERVAL_MS = 15_000

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function useRuntimeStatus(): { refresh: () => Promise<void> } {
  const setHealth = useCodeSessionStore((s) => s.setHealth)
  const setRuntimeInfo = useCodeSessionStore((s) => s.setRuntimeInfo)

  const refresh = useCallback(async () => {
    try {
      const health = await getHealth()
      setHealth({ online: health.status === 'ok' || health.status === 'healthy', version: health.version })
    } catch (error) {
      setHealth({ online: false, error: describe(error) })
      return
    }

    // The runtime-info call is authenticated; a 401 there is informative on its
    // own, so it must not flip the liveness verdict established above.
    try {
      const info = await getRuntimeInfo()
      setRuntimeInfo({ version: info.version, dataDir: info.data_dir, threadCount: info.thread_count })
    } catch {
      setRuntimeInfo({ version: null, dataDir: null, threadCount: null })
    }
  }, [setHealth, setRuntimeInfo])

  useEffect(() => {
    void refresh()
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [refresh])

  return { refresh }
}
