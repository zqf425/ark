/** Child run commands scoped by the parent thread on every request. */
import type {
  ChildRunActionResult,
  ChildRunEventsResponse,
  ChildRunListResponse,
  ThreadDetail
} from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

export function listChildRuns(threadId: string, signal?: AbortSignal): Promise<ChildRunListResponse> {
  return get<ChildRunListResponse>('/v1/child-runs', { query: { thread_id: threadId }, ...(signal ? { signal } : {}) })
}

export function abortChildRun(threadId: string, childId: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/abort`, undefined, { query: { parent_thread_id: threadId } })
}

export function steerChildRun(threadId: string, childId: string, text: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/steer`, { text }, { query: { parent_thread_id: threadId } })
}

export function resumeChildRun(threadId: string, childId: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/resume`, {}, { query: { parent_thread_id: threadId } })
}

/**
 * `GET /v1/child-runs/{id}/thread` — the child's authoritative transcript
 * snapshot (same item projection as a top-level thread).
 */
export function getChildRunThread(threadId: string, childId: string, signal?: AbortSignal): Promise<ThreadDetail> {
  return get<ThreadDetail>(`/v1/child-runs/${childId}/thread`, { query: { parent_thread_id: threadId }, ...(signal ? { signal } : {}) })
}

/**
 * `GET /v1/child-runs/{id}/events` — incremental transcript polling with
 * `since_seq` resume semantics (same watermark rule as the parent SSE).
 */
export function getChildRunEvents(
  threadId: string,
  childId: string,
  sinceSeq: number,
  signal?: AbortSignal
): Promise<ChildRunEventsResponse> {
  return get<ChildRunEventsResponse>(`/v1/child-runs/${childId}/events`, {
    query: { parent_thread_id: threadId, since_seq: sinceSeq },
    ...(signal ? { signal } : {})
  })
}
