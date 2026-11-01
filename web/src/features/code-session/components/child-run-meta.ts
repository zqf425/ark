import type { ChildRunSummary } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type ChildRunMetaPart = { key: MessageKey; params?: MessageParams }

/** Renders only optional facts the runtime recorded for this child run. */
export function childRunMeta(child: ChildRunSummary): ChildRunMetaPart[] {
  const rows: ChildRunMetaPart[] = []
  if (child.model || child.provider_id) rows.push({ key: 'canvas.child.meta.model', params: { model: [child.provider_id, child.model].filter(Boolean).join(' · ') } })
  if (child.tool_invocations !== undefined) rows.push({ key: 'canvas.child.meta.toolInvocations', params: { count: child.tool_invocations } })
  if (child.resume_count !== undefined && child.resume_count > 0) rows.push({ key: 'canvas.child.meta.resumed', params: { count: child.resume_count } })
  return rows
}
