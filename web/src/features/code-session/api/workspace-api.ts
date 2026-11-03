/**
 * Workspace helpers used by the per-session runtime panel.
 *
 * Only the native folder chooser lives here; the full `/v1/workspace/*`
 * surface (status/switch/diff) belongs to the settings feature's api layer.
 */
import type { FolderPickResponse } from '../../../shared/contracts'
import { post } from '../../../shared/api/client'

/**
 * `POST /v1/workspace/pick` — opens the host's native folder dialog and
 * resolves once the user closes it. `path` is undefined when the user
 * cancelled or the host build has no dialog (headless/remote).
 */
export function pickWorkspaceFolder(): Promise<FolderPickResponse> {
  return post<FolderPickResponse>('/v1/workspace/pick', {})
}
