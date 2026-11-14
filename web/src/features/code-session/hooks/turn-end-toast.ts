/**
 * One toast when a session's turn reaches a terminal state.
 *
 * Two callers share this module because a turn's end is observable from two
 * places: the active thread's SSE stream (instant, carries the terminal event
 * itself) and the rail's list poll (background threads and turns started by
 * other windows, up to one interval late). Both pass the same dedupe key —
 * the terminal event's durable `timestamp`, which the list also exposes as
 * `last_event_at` — so a completion is toasted exactly once no matter which
 * surface saw it first: an SSE toast suppresses the poll's later edge, and a
 * poll toast suppresses the replayed terminal frame when the user opens the
 * thread afterwards.
 *
 * Runs outside React (SSE callbacks, poll intervals), so it resolves the
 * toast sink and the thread title from module state instead of hooks.
 */
import type { MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { pushToastProgrammatic } from '../../../shared/ui'
import { useCodeSessionStore } from '../state/store'

export type TurnEndOutcome = 'completed' | 'failed' | 'aborted' | 'ended'

const TONE_BY_OUTCOME: Record<TurnEndOutcome, 'success' | 'error' | 'info'> = {
  completed: 'success',
  failed: 'error',
  aborted: 'info',
  ended: 'info'
}

const MESSAGE_KEY: Record<TurnEndOutcome, MessageKey> = {
  completed: 'thread.notify.completed',
  failed: 'thread.notify.failed',
  aborted: 'thread.notify.aborted',
  ended: 'thread.notify.ended'
}

/** threadId → dedupe key of the last toasted terminal state. */
const notifiedEnds = new Map<string, string>()
/** Bounded like every other client-side bookkeeping map. */
const MAX_NOTIFIED_ENDS = 200

/**
 * Clear the cross-surface dedupe. Test suites call this in `beforeEach` (the
 * module map outlives a single test); production callers have no reason to
 * reset — the keys are immutable durable timestamps.
 */
export function resetTurnEndNotifications(): void {
  notifiedEnds.clear()
}

/**
 * Push the terminal-state toast for one thread unless that exact end was
 * already toasted. `endTime` is the terminal event's durable timestamp (SSE
 * `event.timestamp`, list `last_event_at`); callers without one fall back to
 * `undefined`, which dedupes nothing but still toasts once per call.
 */
export function notifyTurnEnded(payload: {
  threadId: string
  outcome: TurnEndOutcome
  endTime?: string
}): boolean {
  const previous = payload.endTime === undefined ? undefined : notifiedEnds.get(payload.threadId)
  if (previous !== undefined && previous === payload.endTime) return false
  if (notifiedEnds.size >= MAX_NOTIFIED_ENDS) notifiedEnds.clear()
  if (payload.endTime !== undefined) notifiedEnds.set(payload.threadId, payload.endTime)

  const row = useCodeSessionStore.getState().threads.find((entry) => entry.thread_id === payload.threadId)
  if (!row) return false
  pushToastProgrammatic(
    TONE_BY_OUTCOME[payload.outcome],
    translateNow(MESSAGE_KEY[payload.outcome], { title: row.title })
  )
  return true
}
