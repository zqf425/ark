import type { WorkspaceDiff } from '../../../shared/contracts'

export type WorkspaceDiffSummary = {
  staged: string
  unstaged: string
  untracked: string
  truncated: boolean
}

/** Formats the runtime-owned, bounded Git projection without inferring file state. */
export function workspaceDiffSummary(diff: WorkspaceDiff): WorkspaceDiffSummary {
  return {
    staged: `已暂存 ${diff.staged.length}`,
    unstaged: `未暂存 ${diff.unstaged.length}`,
    untracked: `未跟踪 ${diff.untracked.length}`,
    truncated: diff.truncated
  }
}
