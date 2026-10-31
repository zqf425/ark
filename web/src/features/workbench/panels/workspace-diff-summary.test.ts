import { describe, expect, it } from 'vitest'
import type { WorkspaceDiff } from '../../../shared/contracts'
import { translate } from '../../../shared/i18n/messages'
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

function render(value: WorkspaceDiff, locale: 'zh-CN' | 'en-US' = 'zh-CN'): string {
  const summary = workspaceDiffSummary(value)
  return [summary.staged, summary.unstaged, summary.untracked]
    .map((part) => translate(locale, part.key, part.params))
    .join(' / ')
}

describe('workspaceDiffSummary', () => {
  it('separates staged, unstaged, and untracked changes for the user', () => {
    const value = diff({
      staged: [{ path: 'src/a.rs', status: 'M', binary: false, truncated: false }],
      unstaged: [
        { path: 'src/b.rs', status: 'M', binary: false, truncated: false },
        { path: 'src/c.rs', status: 'D', binary: false, truncated: false }
      ],
      untracked: [{ path: 'README.md', status: '?', binary: false, truncated: false }],
      file_count: 4
    })

    expect(render(value)).toBe('已暂存 1 / 未暂存 2 / 未跟踪 1')
    expect(render(value, 'en-US')).toBe('Staged 1 / Unstaged 2 / Untracked 1')
  })

  it('warns when the runtime bounded the result rather than implying completeness', () => {
    expect(workspaceDiffSummary(diff({ truncated: true }))).toMatchObject({ truncated: true })
  })
})
