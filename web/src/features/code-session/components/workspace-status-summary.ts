import type { WorkspaceStatus } from '../../../shared/contracts'

export type WorkspaceStatusSummary = {
  label: string
  detail: string
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

  if (!status.exists) return { label, detail: '工作区路径不存在', tone: 'error' }
  if (!status.is_git_repository) return { label, detail: '不是 Git 仓库', tone: 'warn' }
  if (status.is_dirty === undefined) return { label, detail: '工作区状态未知', tone: 'warn' }
  if (status.is_dirty) {
    const count = status.file_change_count ?? 0
    return { label, detail: count > 0 ? `${count} 项未提交改动` : '有未提交改动', tone: 'warn' }
  }
  return { label, detail: '工作区干净', tone: 'ok' }
}
