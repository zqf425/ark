/**
 * Per-thread runtime overrides for the composer pills.
 *
 * Every pill shows "what this thread's next turn would use", so the value has
 * to belong to the selected thread — not to whatever the pill read the last
 * time its menu happened to open. `ThreadStream` remounts the composer on every
 * switch, which drops any pill-local copy; reading only on menu-open therefore
 * left the label on the runtime default until the operator opened the menu
 * again.
 *
 * The values live in the session store keyed by thread id. Their single writer
 * is the authoritative detail load in `useThreadStream`, which already carries
 * the stored overrides: a pill therefore never fetches the transcript itself,
 * and a switch costs one read no matter how many pills are on screen.
 *
 * A pill never renders another thread's values. While the entry for the thread
 * in view is missing it reports no overrides, which renders the runtime default
 * — the same thing a fresh session shows.
 */
import { useCallback, useRef } from 'react'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { getThread } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import type { RuntimeOwner } from '../components/session-runtime-guard'

/** Shared empty view: a pill with no override renders the runtime default. */
const NO_OVERRIDES: ThreadRuntimeSettings = {}

export type ThreadRuntimeState = {
  /** Overrides of the thread in view; empty until that thread's detail lands. */
  runtime: ThreadRuntimeSettings
  /** Thread whose overrides are shown; `null` while the detail is missing. */
  loadedFor: RuntimeOwner
  /**
   * Re-read the thread now.
   *
   * Called when a picker opens (so a value cached since the selection can never
   * be written back over another editor's change) and after this pill's own
   * PATCH (the route answers with the thread summary, not the merged overrides).
   */
  reload: () => Promise<void>
}

/**
 * `onError` receives reload failures; the caller maps them to its own surface
 * (the composer writes `setCommandError`). A failed reload leaves the entry
 * untouched, so a save still targets the values that were actually read.
 */
export function useThreadRuntime(
  threadId: string | null,
  onError?: (error: unknown) => void
): ThreadRuntimeState {
  const cached = useCodeSessionStore((state) => (threadId ? state.threadRuntime[threadId] : undefined))
  // Latest error sink without rebuilding `reload` on every render.
  const onErrorRef = useRef(onError)
  onErrorRef.current = onError
  // Guards a reload that resolves after the operator already switched threads.
  const currentRef = useRef<string | null>(threadId)
  currentRef.current = threadId

  const reload = useCallback(async (): Promise<void> => {
    if (!threadId) return
    const request = useCodeSessionStore.getState().beginThreadRuntime(threadId)
    try {
      const detail = await getThread(threadId)
      if (currentRef.current === threadId) {
        useCodeSessionStore.getState().setThreadRuntime(threadId, { ...detail.runtime }, request)
      }
    } catch (error) {
      if (
        currentRef.current === threadId &&
        useCodeSessionStore.getState().isThreadRuntimeCurrent(request)
      ) {
        onErrorRef.current?.(error)
      }
    }
  }, [threadId])

  return {
    runtime: cached ?? NO_OVERRIDES,
    loadedFor: threadId && cached !== undefined ? threadId : null,
    reload
  }
}