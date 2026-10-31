/**
 * Rewind orchestration, kept free of React so the decision surface is testable.
 *
 * Availability is derived from the item snapshot, never from a per-turn status
 * table alone: `GET /v1/threads/{id}` returns items but no historical turn
 * states, so a turn that finished in an earlier page load has no `turnState`
 * entry at all. A turn is therefore rewindable when it has items and every one
 * of them is terminal; the whole thread is only rewindable while no turn is
 * live, because the server answers 409 for a thread with a running turn.
 */
import type { RewindRequest, RewindResponse } from '../../../generated'
import type { ThreadId, TurnId, TurnState } from '../../../shared/contracts'
import type { ItemsSnapshot } from '../state/types'

/** `ready` fires, `busy` renders disabled, `hidden` renders nothing. */
export type RewindAvailability = 'ready' | 'busy' | 'hidden'

/** The slice of the snapshot the decision needs; keeps this module pure. */
export type RewindView = Pick<ItemsSnapshot, 'activeTurnId' | 'turnState' | 'itemsById' | 'itemOrder'>

const LIVE_TURN_STATES: ReadonlySet<TurnState> = new Set<TurnState>(['running', 'aborting'])

export function isTerminalTurnState(state: TurnState | undefined): boolean {
  return state === 'completed' || state === 'failed' || state === 'aborted'
}

/**
 * True while the thread cannot be rewound at all.
 *
 * `activeTurnId` comes from the detail runtime snapshot and is the authority;
 * the item statuses are the fallback for a turn whose terminal event has not
 * been folded yet.
 */
export function hasRunningTurn(view: RewindView): boolean {
  if (view.activeTurnId) return true
  for (const state of Object.values(view.turnState)) {
    if (state && LIVE_TURN_STATES.has(state)) return true
  }
  return view.itemOrder.some((itemId) => {
    const item = view.itemsById[itemId]
    return item !== undefined && (item.status === 'pending' || item.status === 'streaming')
  })
}

export function rewindAvailability(view: RewindView, turnId: TurnId): RewindAvailability {
  return rewindAvailabilityByTurn(view).get(turnId) ?? 'hidden'
}

/**
 * Availability for every turn in one pass over the snapshot.
 *
 * The canvas renders one entry per turn, so asking per turn would rescan the
 * whole item list each time (O(turns x items) on a long thread). Callers that
 * render many entries should compute this map once and pass the result down.
 */
export function rewindAvailabilityByTurn(view: RewindView): Map<TurnId, RewindAvailability> {
  const rowCount = new Map<TurnId, number>()
  const liveRow = new Set<TurnId>()

  for (const itemId of view.itemOrder) {
    const item = view.itemsById[itemId]
    if (!item) continue
    rowCount.set(item.turn_id, (rowCount.get(item.turn_id) ?? 0) + 1)
    if (item.status === 'pending' || item.status === 'streaming') liveRow.add(item.turn_id)
  }

  // A live turn anywhere in the thread blocks every entry: the server refuses
  // to rewind a thread with a running turn, not just the running turn itself.
  const blocked = liveRow.size > 0 || hasRunningTurn(view)
  const availability = new Map<TurnId, RewindAvailability>()

  for (const turnId of rowCount.keys()) {
    if (blocked) {
      availability.set(turnId, 'busy')
      continue
    }
    const state = view.turnState[turnId]
    availability.set(turnId, state !== undefined && !isTerminalTurnState(state) ? 'busy' : 'ready')
  }
  return availability
}

/** The server contract allows exactly one of turn_id / seq; the UI uses turn_id. */
export type RewindTarget = { turnId: TurnId }

export type RewindOutcome = {
  rewind: (threadId: ThreadId, request: RewindRequest) => Promise<RewindResponse>
  /** Receives the server watermark; the stream re-seeds from a fresh detail. */
  onRewound: (response: RewindResponse) => void
  onError: (error: unknown) => void
}

export type RewindGuard = {
  threadId: ThreadId
  turnId: TurnId
  availability: RewindAvailability
}

export async function executeRewind(guard: RewindGuard, outcome: RewindOutcome): Promise<boolean> {
  if (!guard.threadId || !guard.turnId || guard.availability !== 'ready') return false
  try {
    const response = await outcome.rewind(guard.threadId, { turn_id: guard.turnId })
    outcome.onRewound(response)
    return true
  } catch (error) {
    outcome.onError(error)
    return false
  }
}
