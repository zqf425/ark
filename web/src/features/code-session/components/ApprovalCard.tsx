import { useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import { Button } from '../../../shared/ui'
import { useToast } from '../../../shared/ui'
import { resolveApproval } from '../api/approvals-api'
import type { PendingApproval } from '../state/types'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  if (error instanceof Error) return error.message
  return String(error)
}

/**
 * Approval gate. The verdict is a command, not a state guess: the resolved
 * status only changes once `approval_resolved` arrives (or the POST succeeds).
 */
export function ApprovalCard({
  approval,
  onResolved
}: {
  approval: PendingApproval
  onResolved: (approvalId: string, approved: boolean) => void
}): JSX.Element {
  const toast = useToast()
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null)
  const decided = approval.status !== 'pending'

  const decide = async (approved: boolean): Promise<void> => {
    setBusy(approved ? 'approve' : 'reject')
    try {
      await resolveApproval(approval.approval_id, approved)
      onResolved(approval.approval_id, approved)
    } catch (error) {
      toast.push('error', '审批提交失败', describe(error))
    } finally {
      setBusy(null)
    }
  }

  return (
    <section className="ark-gate ark-gate--approval">
      <header className="ark-gate__head">
        <span className="ark-gate__title">需要审批</span>
        <code className="ark-gate__tool">{approval.tool}</code>
        {decided ? (
          <span className={`ark-chip ${approval.status === 'approved' ? 'ark-chip--ok' : 'ark-chip--error'}`}>
            {approval.status === 'approved' ? '已批准' : '已拒绝'}
          </span>
        ) : null}
      </header>
      <p className="ark-gate__body">{approval.summary}</p>
      <footer className="ark-gate__actions">
        <Button
          variant="primary"
          size="sm"
          disabled={decided || busy !== null}
          onClick={() => void decide(true)}
        >
          {busy === 'approve' ? '提交中…' : '批准'}
        </Button>
        <Button variant="danger" size="sm" disabled={decided || busy !== null} onClick={() => void decide(false)}>
          {busy === 'reject' ? '提交中…' : '拒绝'}
        </Button>
      </footer>
    </section>
  )
}
