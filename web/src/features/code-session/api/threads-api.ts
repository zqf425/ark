/**
 * Thread surface only. No business rules: it maps HTTP shapes, nothing more.
 */
import type {
  CreateThreadRequest,
  ThreadDetail,
  ThreadId,
  ThreadSummary,
  TurnState,
  UpdateThreadRequest
} from '../../../shared/contracts'
import { del, get, patch, post } from '../../../shared/api/client'

/** `GET /v1/threads` — deterministic order; archived threads are filtered by the caller. */
export function listThreads(signal?: AbortSignal): Promise<ThreadSummary[]> {
  return get<ThreadSummary[]>('/v1/threads', signal ? { signal } : {})
}

/** `GET /v1/threads/{id}` — metadata plus the render-ready item stream. */
export function getThread(threadId: ThreadId, signal?: AbortSignal): Promise<ThreadDetail> {
  return get<ThreadDetail>(`/v1/threads/${threadId}`, signal ? { signal } : {})
}

/** `POST /v1/threads`. The runtime assigns the id. */
export function createThread(request: CreateThreadRequest = {}): Promise<ThreadSummary> {
  return post<ThreadSummary>('/v1/threads', request)
}

/** `PATCH /v1/threads/{id}` — rename / archive. */
export function updateThread(threadId: ThreadId, request: UpdateThreadRequest): Promise<ThreadSummary> {
  return patch<ThreadSummary>(`/v1/threads/${threadId}`, request)
}

/** `DELETE /v1/threads/{id}`. */
export function deleteThread(threadId: ThreadId): Promise<void> {
  return del<void>(`/v1/threads/${threadId}`)
}

/** `POST /v1/threads/{id}/fork` — copies the latest durable event prefix. */
export function forkThread(threadId: ThreadId): Promise<ThreadSummary> {
  return post<ThreadSummary>(`/v1/threads/${threadId}/fork`)
}

/** `POST /v1/threads/{id}/compact` — persist a context checkpoint. */
export function compactThread(threadId: ThreadId): Promise<{ checkpoint: string }> {
  return post<{ checkpoint: string }>(`/v1/threads/${threadId}/compact`)
}

export type { TurnState }
