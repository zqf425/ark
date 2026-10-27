/**
 * Workspace surface (`/v1/workspace/*`).
 *
 * The probe is read-only; `validate` is the runtime's own path check so the UI
 * never re-implements filesystem rules in the browser.
 */
import type { WorkspaceStatus, WorkspaceValidateResult } from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

/** `GET /v1/workspace/status?path=`. */
export function getWorkspaceStatus(path: string, signal?: AbortSignal): Promise<WorkspaceStatus> {
  return get<WorkspaceStatus>('/v1/workspace/status', {
    query: { path },
    ...(signal ? { signal } : {})
  })
}

/** `POST /v1/workspace/validate`. */
export function validateWorkspace(path: string): Promise<WorkspaceValidateResult> {
  return post<WorkspaceValidateResult>('/v1/workspace/validate', { path })
}
