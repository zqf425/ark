/**
 * Turn surface: start / interrupt / read. State transitions live in the loop,
 * so nothing here infers turn state from the response body.
 */
import type { ThreadId, TurnId, TurnRequest, TurnSummary } from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

/** `POST /v1/threads/{id}/turns` — the runtime answers with the started turn. */
export function startTurn(threadId: ThreadId, request: TurnRequest): Promise<TurnSummary> {
  return post<TurnSummary>(`/v1/threads/${threadId}/turns`, request)
}

/** `POST /v1/threads/{id}/turns/{turnId}/interrupt`. */
export function interruptTurn(threadId: ThreadId, turnId: TurnId): Promise<void> {
  return post<void>(`/v1/threads/${threadId}/turns/${turnId}/interrupt`)
}

/**
 * `POST /v1/threads/{id}/turns/{turnId}/steer`. The body is the same
 * `TurnRequest` envelope as a turn start: attachment ids are optional and must
 * already be associated with the thread (the route authorizes them).
 */
export function steerTurn(
  threadId: ThreadId,
  turnId: TurnId,
  request: Pick<TurnRequest, 'text' | 'attachment_ids'>
): Promise<void> {
  return post<void>(`/v1/threads/${threadId}/turns/${turnId}/steer`, request)
}

/** `GET /v1/threads/{id}/turns/{turnId}`. */
export function getTurn(threadId: ThreadId, turnId: TurnId, signal?: AbortSignal): Promise<TurnSummary> {
  return get<TurnSummary>(`/v1/threads/${threadId}/turns/${turnId}`, signal ? { signal } : {})
}

/**
 * `POST /v1/threads/{id}/review` — a system-composed review turn seeded with
 * the thread's workspace-change projection. The response is the same 202
 * `TurnSummary` envelope as a normal turn; progress arrives on the event
 * stream under the returned turn id. An empty body means "no focus".
 */
export function startReview(threadId: ThreadId): Promise<TurnSummary> {
  return post<TurnSummary>(`/v1/threads/${threadId}/review`, {})
}
