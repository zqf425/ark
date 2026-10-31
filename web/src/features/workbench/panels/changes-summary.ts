import type { WorkspaceChange } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type ChangesSummaryPart = { key: MessageKey; params: MessageParams }

/** Summarizes only runtime-projected successful file mutations. */
export function changesSummary(changes: WorkspaceChange[]): ChangesSummaryPart[] {
  const writes = changes.filter((change) => change.operation === 'write').length
  const edits = changes.filter((change) => change.operation === 'edit').length
  const known = writes + edits
  const parts: ChangesSummaryPart[] = [{ key: 'workbench.changesSummary.files', params: { count: changes.length } }]
  if (writes > 0) parts.push({ key: 'workbench.changesSummary.writes', params: { count: writes } })
  if (edits > 0) parts.push({ key: 'workbench.changesSummary.edits', params: { count: edits } })
  if (known < changes.length) {
    parts.push({ key: 'workbench.changesSummary.operations', params: { count: changes.length - known } })
  }
  return parts
}
