/**
 * Per-turn rewind entry.
 *
 * Rendered on every turn that still has rows. A quiescent thread shows an
 * actionable control; a thread with a live turn shows the same control disabled,
 * because the server answers 409 for it (docs/modules/web.md: the entry must not
 * claim an operation the runtime will refuse). The confirm step states the
 * destructive scope before anything is sent. Success re-seeds the stream from a
 * fresh detail watermark through the reload token, so deleted turns and their
 * derived gates/errors disappear with the truncated history.
 */
import { useState } from 'react'
import type { ThreadId, TurnId } from '../../../shared/contracts'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { rewindThread } from '../api/rewind-api'
import { useCodeSessionStore } from '../state/store'
import { executeRewind, type RewindAvailability } from './rewind-flow'

/**
 * The server's own message is the only accurate explanation for a refusal
 * (409 running turn, 404 missing target), so it is shown verbatim after the
 * localized hint. Nothing here invents a client-side reason.
 */
function describe(error: unknown): string {
  return describeError(error, translateNow)
}

/**
 * `availability` is computed once per snapshot by the canvas: deriving it here
 * would rescan the item list for every turn on the thread.
 */
export function RewindControl({
  threadId,
  turnId,
  availability
}: {
  threadId: ThreadId
  turnId: TurnId
  availability: RewindAvailability
}): JSX.Element | null {
  const { t } = useI18n()
  const { push } = useToast()
  const [confirming, setConfirming] = useState(false)
  const [pending, setPending] = useState(false)

  const requestStreamReload = useCodeSessionStore((s) => s.requestStreamReload)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)

  if (availability === 'hidden') return null

  const busy = availability !== 'ready'
  const run = async (): Promise<void> => {
    setPending(true)
    setCommandError(null)
    const ok = await executeRewind(
      { threadId, turnId, availability },
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
        <Button
          variant="ghost"
          size="sm"
          disabled={busy}
          title={busy ? t('session.rewind.busyHint') : t('session.rewind.confirmBody')}
          onClick={() => setConfirming(true)}
        >
          {t('session.rewind.entry')}
        </Button>
      </div>
    )
  }

  return (
    <div className="ark-rewind ark-rewind--confirm" role="alertdialog" aria-label={t('session.rewind.confirmTitle')}>
      <p className="ark-rewind__title">{t('session.rewind.confirmTitle')}</p>
      <p className="ark-rewind__body">{t('session.rewind.confirmBody')}</p>
      <p className="ark-rewind__body">{t('session.rewind.confirmWorkspace')}</p>
      <div className="ark-rewind__actions">
        <Button variant="primary" size="sm" disabled={pending || busy} onClick={() => void run()}>
          {pending ? t('session.rewind.rewinding') : t('session.rewind.confirmAction')}
        </Button>
        <Button variant="ghost" size="sm" disabled={pending} onClick={() => setConfirming(false)}>
          {t('session.rewind.cancel')}
        </Button>
      </div>
    </div>
  )
}
