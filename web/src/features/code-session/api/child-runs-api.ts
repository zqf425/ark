/** Child run commands scoped by the parent thread on every request. */
import type { ChildRunActionResult, ChildRunListResponse } from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

export function listChildRuns(threadId: string, signal?: AbortSignal): Promise<ChildRunListResponse> {
  return get<ChildRunListResponse>('/v1/child-runs', { query: { threadId }, ...(signal ? { signal } : {}) })
}

export function abortChildRun(threadId: string, childId: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/abort`, undefined, { query: { parentThreadId: threadId } })
}

export function steerChildRun(threadId: string, childId: string, text: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/steer`, { text }, { query: { parentThreadId: threadId } })
}

export function resumeChildRun(threadId: string, childId: string): Promise<ChildRunActionResult> {
  return post<ChildRunActionResult>(`/v1/child-runs/${childId}/resume`, {}, { query: { parentThreadId: threadId } })
}
