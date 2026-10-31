import { describe, expect, it } from 'vitest'
import type { WorkspaceStatus } from '../../../shared/contracts'
import { translate } from '../../../shared/i18n/messages'
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

/** Resolve the localized detail the same way the status bar does. */
function detailOf(value: WorkspaceStatus, locale: 'zh-CN' | 'en-US' = 'zh-CN'): string {
  const summary = workspaceStatusSummary(value)
  return translate(locale, summary.detail.key, summary.detail.params)
}

describe('workspaceStatusSummary', () => {
  it('renders a clean git workspace with its directory and branch', () => {
    const summary = workspaceStatusSummary(status())

    expect(summary.label).toBe('ark · master')
    expect(detailOf(status())).toBe('工作区干净')
    expect(summary.tone).toBe('ok')
  })

  it('makes uncommitted change count visible', () => {
    const summary = workspaceStatusSummary(status({ is_dirty: true, file_change_count: 3 }))

    expect(summary.label).toBe('ark · master')
    expect(detailOf(status({ is_dirty: true, file_change_count: 3 }))).toBe('3 项未提交改动')
    expect(summary.tone).toBe('warn')
  })

  it('does not present a missing directory as a usable workspace', () => {
    const summary = workspaceStatusSummary(status({ path: 'D:/missing', exists: false, is_git_repository: false }))

    expect(summary.label).toBe('missing')
    expect(detailOf(status({ path: 'D:/missing', exists: false, is_git_repository: false }))).toBe('工作区路径不存在')
    expect(summary.tone).toBe('error')
  })

  it('does not claim the workspace is clean when git did not report dirtiness', () => {
    const summary = workspaceStatusSummary(status({ is_dirty: undefined, file_change_count: undefined }))

    expect(summary.label).toBe('ark · master')
    expect(detailOf(status({ is_dirty: undefined, file_change_count: undefined }))).toBe('工作区状态未知')
    expect(summary.tone).toBe('warn')
  })

  it('localizes the same verdicts into English', () => {
    expect(detailOf(status(), 'en-US')).toBe('Workspace clean')
    expect(detailOf(status({ is_dirty: true, file_change_count: 3 }), 'en-US')).toBe('3 uncommitted changes')
    expect(detailOf(status({ exists: false }), 'en-US')).toBe('The workspace path does not exist')
  })
})
