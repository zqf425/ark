import { useEffect, useRef } from 'react'

/**
 * The one mounted-guard every data hook needs: a ref that is true for the
 * component's live lifetime, so a settled request can tell whether its setState
 * calls still have an owner.
 *
 * React 18 no longer warns on updates to unmounted components, but the guard
 * stays: applying a stale response would still clobber a newer one that landed
 * between the await and the wakeup.
 */
export function useMounted(): { current: boolean } {
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  return mounted
}
