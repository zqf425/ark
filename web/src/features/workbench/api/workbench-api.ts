/** Typed HTTP surface for the Code workbench side panels. */
import type {
  FilePreview,
  GoalRequest,
  PlanView,
  TerminalEntriesResponse,
  TodosRequest,
  WorkspaceChangesResponse,
  WorkspaceFilePreview
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

/**
 * `GET /v1/workspace/file?path=&thread_id=` — a bounded, read-only preview of
 * one file under the workspace root that authorizes `threadId`
 * (requirements/workspace-file-preview.md). Unlike the thread projection above,
 * this answers for any file under the root, read or not.
 *
 * `threadId` is what makes the root *this session's*: the runtime resolves it
 * exactly as a turn does (session override first, process root second). Without
 * it the preview would be authorized against the process-wide root only, and a
 * session bound to another project could not open its own files.
 */
export function getWorkspaceFilePreview(path: string, threadId: string, signal?: AbortSignal): Promise<WorkspaceFilePreview> {
  return get<WorkspaceFilePreview>('/v1/workspace/file', {
    query: { path, thread_id: threadId },
    ...(signal ? { signal } : {})
  })
}

export function getTerminalEntries(threadId: string, signal?: AbortSignal): Promise<TerminalEntriesResponse> {
  return get<TerminalEntriesResponse>(`/v1/threads/${threadId}/terminal`, signal ? { signal } : {})
}
