import type { ChildRunSummary } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type ChildRunMetaPart = { key: MessageKey; params?: MessageParams }

/** Renders only optional facts the runtime recorded for this child run. */
export function childRunMeta(child: ChildRunSummary): ChildRunMetaPart[] {
  const rows: ChildRunMetaPart[] = []
  if (child.model || child.providerId) rows.push({ key: 'canvas.child.meta.model', params: { model: [child.providerId, child.model].filter(Boolean).join(' · ') } })
  if (child.toolInvocations !== undefined) rows.push({ key: 'canvas.child.meta.toolInvocations', params: { count: child.toolInvocations } })
  if (child.resumeCount !== undefined && child.resumeCount > 0) rows.push({ key: 'canvas.child.meta.resumed', params: { count: child.resumeCount } })
  return rows
}
