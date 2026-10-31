/**
 * Rewind orchestration, kept free of React so the decision surface is testable.
 *
 * Guard: the server answers 409 when any turn is still running, so the client
 * refuses to fire when the local snapshot shows live turns. Errors (409/404 and
 * anything else) go to the caller's error channel; success reports the new
 * `last_seq` watermark so the stream can re-seed from a fresh detail.
 */
import type { RewindRequest, RewindResponse } from '../../../generated'
import type { ThreadId, TurnId, TurnState } from '../../../shared/contracts'

/** Only terminal turns may serve as a rewind target. */
export function canRewindTurn(state: TurnState | undefined): boolean {
  return state === 'completed' || state === 'failed' || state === 'aborted'
}

/** The server contract allows exactly one of turn_id / seq. */
export type RewindTarget = { turnId: TurnId }

export type RewindOutcome = {
  rewind: (threadId: ThreadId, request: RewindRequest) => Promise<RewindResponse>
  onRewound: (lastSeq: number) => void
  onError: (error: unknown) => void
}

export type RewindGuard = {
  threadId: ThreadId
  turnId: TurnId
  activeTurnId: TurnId | null
  running: boolean
}

export async function executeRewind(guard: RewindGuard, outcome: RewindOutcome): Promise<boolean> {
  if (!guard.threadId || !guard.turnId || guard.running || guard.activeTurnId) return false
  try {
    const response = await outcome.rewind(guard.threadId, { turn_id: guard.turnId })
    outcome.onRewound(response.last_seq)
    return true
  } catch (error) {
    outcome.onError(error)
    return false
  }
}
