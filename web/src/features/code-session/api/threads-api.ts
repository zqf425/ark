/**
 * Thread surface only. No business rules: it maps HTTP shapes, nothing more.
 */
import type {
  BulkDeleteThreadsResponse,
  CreateThreadRequest,
  ThreadDetail,
  ThreadId,
  ThreadSummary,
  ThreadUsageResponse,
  TurnState,
  UpdateThreadRequest
} from '../../../shared/contracts'
import { buildUrl, del, get, patch, post, readToken } from '../../../shared/api/client'
import { isDesktopShell, pickFolder, saveFile } from '../../../shared/desktop'

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

/**
 * `DELETE /v1/threads?workspace=` — bulk delete of one project's settled
 * sessions. The server decides "settled" (registry + durable log fold) and
 * refuses running threads per-row, so the renderer never races a turn start.
 */
export function deleteThreadsByWorkspace(workspace: string): Promise<BulkDeleteThreadsResponse> {
  return del<BulkDeleteThreadsResponse>('/v1/threads', { query: { workspace } })
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

/** `GET /v1/threads/{id}/usage` — aggregate folded from the thread's usage events. */
export function getThreadUsage(threadId: ThreadId, signal?: AbortSignal): Promise<ThreadUsageResponse> {
  return get<ThreadUsageResponse>(`/v1/threads/${threadId}/usage`, signal ? { signal } : {})
}

export type { TurnState }

/**
 * `GET /v1/threads/{id}/export?format=zip` — save the thread snapshot zip.
 *
 * The backend wraps the same deterministic snapshot JSON into a single-entry
 * zip, so the response is a binary attachment, not an envelope: this bypasses
 * the JSON helpers and attaches the auth header the way `client.ts` does.
 *
 * Inside the desktop shell the user first picks the target folder natively and
 * the shell writes the file there; a cancelled picker aborts the export. In a
 * plain browser the zip falls back to a normal download.
 */
export async function exportThread(threadId: ThreadId, title: string): Promise<void> {
  const token = readToken()
  const headers: Record<string, string> = {}
  if (token) headers.authorization = `Bearer ${token}`
  const response = await fetch(buildUrl(`/v1/threads/${threadId}/export?format=zip`), { headers })
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  const blob = await response.blob()
  const safe = title.replace(/[^\w\u4e00-\u9fff.-]+/g, '_').slice(0, 60) || 'thread'
  const filename = `${safe}-${threadId}.zip`

  if (isDesktopShell()) {
    const folder = await pickFolder()
    if (!folder) return
    await saveFile(`${folder}/${filename}`, new Uint8Array(await blob.arrayBuffer()))
    return
  }

  const url = URL.createObjectURL(blob)
  try {
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = filename
    anchor.click()
  } finally {
    URL.revokeObjectURL(url)
  }
}
