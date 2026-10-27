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

/** `POST /v1/threads/{id}/turns/{turnId}/steer`. */
export function steerTurn(threadId: ThreadId, turnId: TurnId, text: string): Promise<void> {
  return post<void>(`/v1/threads/${threadId}/turns/${turnId}/steer`, { text })
}

/** `GET /v1/threads/{id}/turns/{turnId}`. */
export function getTurn(threadId: ThreadId, turnId: TurnId, signal?: AbortSignal): Promise<TurnSummary> {
  return get<TurnSummary>(`/v1/threads/${threadId}/turns/${turnId}`, signal ? { signal } : {})
}
