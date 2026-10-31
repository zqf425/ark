import type { WorkspaceDiff } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type DiffSummaryPart = { key: MessageKey; params: MessageParams }

export type WorkspaceDiffSummary = {
  staged: DiffSummaryPart
  unstaged: DiffSummaryPart
  untracked: DiffSummaryPart
  truncated: boolean
}

/** Formats the runtime-owned, bounded Git projection without inferring file state. */
export function workspaceDiffSummary(diff: WorkspaceDiff): WorkspaceDiffSummary {
  return {
    staged: { key: 'workbench.diffSummary.staged', params: { count: diff.staged.length } },
    unstaged: { key: 'workbench.diffSummary.unstaged', params: { count: diff.unstaged.length } },
    untracked: { key: 'workbench.diffSummary.untracked', params: { count: diff.untracked.length } },
    truncated: diff.truncated
  }
}
