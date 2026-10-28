import type { WorkspaceChange } from '../../../shared/contracts'

/** Summarizes only runtime-projected successful file mutations. */
export function changesSummary(changes: WorkspaceChange[]): string {
  const writes = changes.filter((change) => change.operation === 'write').length
  const edits = changes.filter((change) => change.operation === 'edit').length
  const known = writes + edits
  const parts = [`${changes.length} 个文件`]
  if (writes > 0) parts.push(`${writes} 个写入文件`)
  if (edits > 0) parts.push(`${edits} 个编辑文件`)
  if (known < changes.length) parts.push(`${changes.length - known} 项操作`)
  return parts.join(' · ')
}
