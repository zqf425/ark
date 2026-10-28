import type { ChildRunSummary } from '../../../shared/contracts'

/** Renders only optional facts the runtime recorded for this child run. */
export function childRunMeta(child: ChildRunSummary): string[] {
  const rows: string[] = []
  if (child.model || child.providerId) rows.push([child.providerId, child.model].filter(Boolean).join(' · '))
  if (child.toolInvocations !== undefined) rows.push(`${child.toolInvocations} 次工具调用`)
  if (child.resumeCount !== undefined && child.resumeCount > 0) rows.push(`已恢复 ${child.resumeCount} 次`)
  return rows
}
