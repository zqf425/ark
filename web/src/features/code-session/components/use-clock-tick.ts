/**
 * A one-second wall-clock tick, frozen when the caller does not need it.
 *
 * Two surfaces render a live active-span clock (the running turn's process
 * header and the session strip's total). Each previously had its own
 * `setInterval` (LiveActivity's pattern); this hook is that loop's only home,
 * so the cadence and the freeze rule stay identical everywhere.
 *
 * `enabled=false` returns a stable timestamp: no timer runs, so a settled
 * turn's re-render never re-ticks the clock.
 */
import { useEffect, useState } from 'react'

export function useClockTick(enabled: boolean): number {
  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    if (!enabled) return
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [enabled])
  return nowMs
}
