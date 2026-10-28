/** Typed HTTP surface for the Code workbench side panels. */
import type {
  FilePreview,
  GoalRequest,
  PlanView,
  TerminalEntriesResponse,
  TodosRequest,
  WorkspaceChangesResponse
} from '../../../shared/contracts'
import { del, get, post } from '../../../shared/api/client'

export function getPlan(threadId: string, signal?: AbortSignal): Promise<PlanView> {
  return get<PlanView>(`/v1/threads/${threadId}/plan`, signal ? { signal } : {})
}

export function updateGoal(threadId: string, goal: string): Promise<PlanView> {
  const body: GoalRequest = { goal }
  return post<PlanView>(`/v1/threads/${threadId}/goal`, body)
}

export function clearGoal(threadId: string): Promise<PlanView> {
  return del<PlanView>(`/v1/threads/${threadId}/goal`)
}

export function updateTodos(threadId: string, todos: TodosRequest): Promise<PlanView> {
  return post<PlanView>(`/v1/threads/${threadId}/todos`, todos)
}

export function getChanges(threadId: string, signal?: AbortSignal): Promise<WorkspaceChangesResponse> {
  return get<WorkspaceChangesResponse>(`/v1/threads/${threadId}/changes`, signal ? { signal } : {})
}

export function getFilePreview(threadId: string, path: string, signal?: AbortSignal): Promise<FilePreview> {
  return get<FilePreview>(`/v1/threads/${threadId}/files`, { query: { path }, ...(signal ? { signal } : {}) })
}

export function getTerminalEntries(threadId: string, signal?: AbortSignal): Promise<TerminalEntriesResponse> {
  return get<TerminalEntriesResponse>(`/v1/threads/${threadId}/terminal`, signal ? { signal } : {})
}
