import type { ChildRunSummary } from '../../../shared/contracts'
import type { MessageKey, MessageParams } from '../../../shared/i18n'

export type ChildRunMetaPart = { key: MessageKey; params?: MessageParams }

/**
 * Duration of one child run, from the runtime's own timestamps. A running run
 * measures up to now; a settled one freezes at `ended_at - created_at`.
 */
export function childRunDuration(child: ChildRunSummary, now = Date.now()): number | null {
  const start = new Date(child.created_at).getTime()
  if (Number.isNaN(start)) return null
  const end = child.ended_at ? new Date(child.ended_at).getTime() : now
  if (Number.isNaN(end) || end < start) return null
  return end - start
}

/**
 * Seconds render as a compact `2m 05s` / `14s` clock so a row can answer "how
 * long has this been going" without a tooltip. Mirrors the frozen-at-finish
 * age row mainstream agent task panels use.
 */
export function formatChildRunDuration(durationMs: number): string {
  const totalSec = Math.max(0, Math.floor(durationMs / 1000))
  const min = Math.floor(totalSec / 60)
  const sec = totalSec % 60
  return min > 0 ? `${min}m ${String(sec).padStart(2, '0')}s` : `${sec}s`
}

/** Renders only optional facts the runtime recorded for this child run. */
export function childRunMeta(child: ChildRunSummary): ChildRunMetaPart[] {
  const rows: ChildRunMetaPart[] = []
  if (child.model || child.provider_id) rows.push({ key: 'canvas.child.meta.model', params: { model: [child.provider_id, child.model].filter(Boolean).join(' · ') } })
  if (child.tool_invocations !== undefined) rows.push({ key: 'canvas.child.meta.toolInvocations', params: { count: child.tool_invocations } })
  if (child.resume_count !== undefined && child.resume_count > 0) rows.push({ key: 'canvas.child.meta.resumed', params: { count: child.resume_count } })
  return rows
}
