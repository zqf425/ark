/**
 * Per-turn rewind entry.
 *
 * Visible only on terminal turns of a quiescent thread. The confirm step states
 * the destructive scope before anything is sent; success re-seeds the stream
 * from a fresh detail (via the reload token) so deleted turns and their
 * derived gates/errors disappear with the truncated history.
 */
import { useState } from 'react'
import type { ThreadId, TurnId } from '../../../shared/contracts'
import { ApiError } from '../../../shared/api/client'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { rewindThread } from '../api/rewind-api'
import { useCodeSessionStore } from '../state/store'
import { canRewindTurn, executeRewind } from './rewind-flow'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  if (error instanceof Error) return error.message
  return String(error)
}

export function RewindControl({ threadId, turnId }: { threadId: ThreadId; turnId: TurnId }): JSX.Element | null {
  const { t } = useI18n()
  const { push } = useToast()
  const [confirming, setConfirming] = useState(false)
  const [pending, setPending] = useState(false)

  const state = useCodeSessionStore((s) => s.turnState[turnId])
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const requestStreamReload = useCodeSessionStore((s) => s.requestStreamReload)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)

  // Terminal turns only, and never while any turn is running: the server would
  // answer 409 anyway, but the entry stays out of the layout entirely.
  if (!canRewindTurn(state) || activeTurnId) return null

  const run = async (): Promise<void> => {
    setPending(true)
    setCommandError(null)
    const ok = await executeRewind(
      { threadId, turnId, activeTurnId, running: false },
      {
        rewind: rewindThread,
        onRewound: () => {
          requestStreamReload()
          push('success', t('session.rewind.done'))
        },
        onError: (error) => {
          const text = describe(error)
          setCommandError(`${t('session.rewind.failed')}：${text}`)
          push('error', t('session.rewind.failed'), text)
        }
      }
    )
    setPending(false)
    if (ok) setConfirming(false)
  }

  if (!confirming) {
    return (
      <div className="ark-rewind">
        <Button variant="ghost" size="sm" onClick={() => setConfirming(true)}>
          {t('session.rewind.entry')}
        </Button>
      </div>
    )
  }

  return (
    <div className="ark-rewind ark-rewind--confirm" role="alertdialog" aria-label={t('session.rewind.confirmTitle')}>
      <p className="ark-rewind__title">{t('session.rewind.confirmTitle')}</p>
      <p className="ark-rewind__body">{t('session.rewind.confirmBody')}</p>
      <div className="ark-rewind__actions">
        <Button variant="primary" size="sm" disabled={pending} onClick={() => void run()}>
          {pending ? t('session.rewind.rewinding') : t('session.rewind.confirmAction')}
        </Button>
        <Button variant="ghost" size="sm" disabled={pending} onClick={() => setConfirming(false)}>
          {t('session.rewind.cancel')}
        </Button>
      </div>
    </div>
  )
}
