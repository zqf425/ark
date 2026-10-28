import { describe, expect, it } from 'vitest'
import type { WorkspaceStatus } from '../../../shared/contracts'
import { workspaceStatusSummary } from './workspace-status-summary'

function status(overrides: Partial<WorkspaceStatus> = {}): WorkspaceStatus {
  return {
    path: 'D:/code/ark',
    exists: true,
    is_git_repository: true,
    branch: 'master',
    head_sha: '0123456789abcdef',
    is_dirty: false,
    file_change_count: 0,
    checked_at: '2026-10-28T00:00:00Z',
    ...overrides
  }
}

describe('workspaceStatusSummary', () => {
  it('renders a clean git workspace with its directory and branch', () => {
    expect(workspaceStatusSummary(status())).toEqual({
      label: 'ark · master',
      detail: '工作区干净',
      tone: 'ok'
    })
  })

  it('makes uncommitted change count visible', () => {
    expect(workspaceStatusSummary(status({ is_dirty: true, file_change_count: 3 }))).toEqual({
      label: 'ark · master',
      detail: '3 项未提交改动',
      tone: 'warn'
    })
  })

  it('does not present a missing directory as a usable workspace', () => {
    expect(workspaceStatusSummary(status({ path: 'D:/missing', exists: false, is_git_repository: false }))).toEqual({
      label: 'missing',
      detail: '工作区路径不存在',
      tone: 'error'
    })
  })

  it('does not claim the workspace is clean when git did not report dirtiness', () => {
    expect(workspaceStatusSummary(status({ is_dirty: undefined, file_change_count: undefined }))).toEqual({
      label: 'ark · master',
      detail: '工作区状态未知',
      tone: 'warn'
    })
  })
})
