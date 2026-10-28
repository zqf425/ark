/**
 * Workspace surface (`/v1/workspace/*`).
 *
 * The probe is read-only; `validate` is the runtime's own path check so the UI
 * never re-implements filesystem rules in the browser.
 */
import type { SwitchWorkspaceRequest, WorkspaceDiff, WorkspaceStatus, WorkspaceValidateResult, WorkspaceView } from '../../../shared/contracts'
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

/** `GET /v1/workspace` — host-owned active workspace and process-local history. */
export function getWorkspaceView(signal?: AbortSignal): Promise<WorkspaceView> {
  return get<WorkspaceView>('/v1/workspace', signal ? { signal } : {})
}

/** `POST /v1/workspace/switch` — rejected while any parent or child turn is active. */
export function switchWorkspace(request: SwitchWorkspaceRequest): Promise<WorkspaceView> {
  return post<WorkspaceView>('/v1/workspace/switch', request)
}

/** `GET /v1/workspace/diff` — bounded Git projection for the active canonical root. */
export function getWorkspaceDiff(signal?: AbortSignal): Promise<WorkspaceDiff> {
  return get<WorkspaceDiff>('/v1/workspace/diff', signal ? { signal } : {})
}
