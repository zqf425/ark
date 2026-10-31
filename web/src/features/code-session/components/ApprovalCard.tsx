import { useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { Button } from '../../../shared/ui'
import { useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { resolveApproval } from '../api/approvals-api'
import type { PendingApproval } from '../state/types'

function describe(error: unknown): string {
  return describeError(error, translateNow)
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
  const { t } = useI18n()
  const toast = useToast()
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null)
  const decided = approval.status !== 'pending'

  const decide = async (approved: boolean): Promise<void> => {
    setBusy(approved ? 'approve' : 'reject')
    try {
      await resolveApproval(approval.approval_id, approved)
      onResolved(approval.approval_id, approved)
    } catch (error) {
      toast.push('error', t('canvas.approval.failed'), describe(error))
    } finally {
      setBusy(null)
    }
  }

  return (
    <section className="ark-gate ark-gate--approval">
      <header className="ark-gate__head">
        <span className="ark-gate__title">{t('canvas.approval.title')}</span>
        <code className="ark-gate__tool">{approval.tool}</code>
        {decided ? (
          <span className={`ark-chip ${approval.status === 'approved' ? 'ark-chip--ok' : 'ark-chip--error'}`}>
            {approval.status === 'approved' ? t('canvas.approval.approved') : t('canvas.approval.rejected')}
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
          {busy === 'approve' ? t('common.submitting') : t('canvas.approval.approve')}
        </Button>
        <Button variant="danger" size="sm" disabled={decided || busy !== null} onClick={() => void decide(false)}>
          {busy === 'reject' ? t('common.submitting') : t('canvas.approval.reject')}
        </Button>
      </footer>
    </section>
  )
}
