import type { WorkspaceStatus } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type WorkspaceStatusSummary = {
  label: string
  detail: { key: MessageKey; params?: MessageParams }
  tone: 'ok' | 'warn' | 'error'
}

function workspaceName(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '')
  return normalized.split(/[\\/]/).at(-1) || path
}

/** Turns the runtime-owned workspace probe into compact, user-facing status. */
export function workspaceStatusSummary(status: WorkspaceStatus): WorkspaceStatusSummary {
  const label = status.is_git_repository && status.branch
    ? `${workspaceName(status.path)} · ${status.branch}`
    : workspaceName(status.path)

  if (!status.exists) return { label, detail: { key: 'canvas.workspace.missing' }, tone: 'error' }
  if (!status.is_git_repository) return { label, detail: { key: 'canvas.workspace.notGit' }, tone: 'warn' }
  if (status.is_dirty === undefined) return { label, detail: { key: 'canvas.workspace.unknown' }, tone: 'warn' }
  if (status.is_dirty) {
    const count = status.file_change_count ?? 0
    return {
      label,
      detail: count > 0
        ? { key: 'canvas.workspace.dirtyCount', params: { count } }
        : { key: 'canvas.workspace.dirty' },
      tone: 'warn'
    }
  }
  return { label, detail: { key: 'canvas.workspace.clean' }, tone: 'ok' }
}
