/**
 * One toast when a session's turn reaches a terminal state — plus, while the
 * window is not focused, a system notification for the same end.
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
 * The system notification rides the same edge and dedupe: when the user is
 * reading another app the in-page toast is invisible, so the desktop shell
 * posts an OS-level one (Windows toast / macOS banner) instead. Focus is read
 * at delivery time, not via listeners — a focus change between the terminal
 * event and this call is accounted for by the next call, and one missed edge
 * costs less than a focus-listener lifecycle around the two entry points.
 * Outside the desktop shell (plain browser) it degrades to the toast only.
 *
 * Runs outside React (SSE callbacks, poll intervals), so it resolves the
 * toast sink and the thread title from module state instead of hooks.
 */
import type { MessageKey } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { postSystemNotification } from '../../../shared/desktop'
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

/** System-notification title per outcome; the toast text stays the body. */
const OS_TITLE_KEY: Record<TurnEndOutcome, MessageKey> = {
  completed: 'thread.notify.os.completed',
  failed: 'thread.notify.os.failed',
  aborted: 'thread.notify.os.aborted',
  ended: 'thread.notify.os.ended'
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
  notifySystem(payload.outcome, row.title)
  return true
}

/**
 * Fire-and-forget OS notification for one deduped turn end. Deliberately not
 * awaited: the toast return value must stay synchronous for the two callers,
 * and a slow shell round-trip must not delay the stream/poll loop. Failure
 * resolves inside `postSystemNotification` and simply leaves the toast alone.
 */
function notifySystem(outcome: TurnEndOutcome, title: string): void {
  if (document.hasFocus()) return
  void postSystemNotification(
    translateNow(OS_TITLE_KEY[outcome]),
    translateNow(MESSAGE_KEY[outcome], { title })
  )
}
