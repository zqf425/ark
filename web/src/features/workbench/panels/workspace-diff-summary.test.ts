import { describe, expect, it } from 'vitest'
import type { WorkspaceDiff } from '../../../shared/contracts'
import { workspaceDiffSummary } from './workspace-diff-summary'

function diff(overrides: Partial<WorkspaceDiff> = {}): WorkspaceDiff {
  return {
    generation: 3,
    staged: [],
    unstaged: [],
    untracked: [],
    file_count: 0,
    patch_bytes: 0,
    truncated: false,
    ...overrides
  }
}

describe('workspaceDiffSummary', () => {
  it('separates staged, unstaged, and untracked changes for the user', () => {
    expect(workspaceDiffSummary(diff({
      staged: [{ path: 'src/a.rs', status: 'M', binary: false, truncated: false }],
      unstaged: [
        { path: 'src/b.rs', status: 'M', binary: false, truncated: false },
        { path: 'src/c.rs', status: 'D', binary: false, truncated: false }
      ],
      untracked: [{ path: 'README.md', status: '?', binary: false, truncated: false }],
      file_count: 4
    }))).toEqual({ staged: '已暂存 1', unstaged: '未暂存 2', untracked: '未跟踪 1', truncated: false })
  })

  it('warns when the runtime bounded the result rather than implying completeness', () => {
    expect(workspaceDiffSummary(diff({ truncated: true }))).toMatchObject({ truncated: true })
  })
})
