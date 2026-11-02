/**
 * WeChat QR login dialog: fetch a QR session, poll until confirmed.
 *
 * A confirmed login creates the channel server-side (disabled); the dialog
 * only surfaces it. Session expiry and failure both stop the poll.
 *
 * Rendering mirrors smart-ark: the iLink payload carries either a ready-made
 * image (`data:image/...`) or a plain login URL — the former is shown as-is,
 * the latter must be encoded into a QR code, or the user sees a bare string
 * instead of something scannable.
 */
import { useEffect, useRef, useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ImWeixinLoginStatus } from '../../../shared/contracts'
import { useIm } from '../hooks/use-im'
import { describeOutcome } from '../api/outcome'
import { Notice, Skeleton } from './SectionShell'

const LOGIN_POLL_INTERVAL_MS = 2000
const LOGIN_POLL_MAX_ATTEMPTS = 150
const QR_SIZE_PX = 192

/** smart-ark's rule: a `data:image/` payload is the picture itself, anything else is the content to encode. */
function qrIsImage(content: string): boolean {
  return content.startsWith('data:image/')
}

export function WeixinLoginDialog({
  onClose
}: {
  onClose: () => void
}): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const im = useIm()
  const [qrContent, setQrContent] = useState<string | null>(null)
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [status, setStatus] = useState<ImWeixinLoginStatus | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [confirmed, setConfirmed] = useState(false)
  const attemptsRef = useRef(0)

  useEffect(() => {
    let cancelled = false
    void im
      .loginStart()
      .then((outcome) => {
        if (cancelled) return
        if (!outcome.ok) {
          setMessage(describeOutcome(outcome))
          return
        }
        setQrContent(outcome.value.qrContent)
        setSessionId(outcome.value.sessionId)
      })
      .catch((thrown) => {
        if (!cancelled) setMessage(String(thrown))
      })
    return () => {
      cancelled = true
    }
    // The login session is fetched once per dialog open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!sessionId || confirmed) return
    if (status === 'failed' || status === 'expired') return
    const timer = setInterval(() => {
      attemptsRef.current += 1
      if (attemptsRef.current > LOGIN_POLL_MAX_ATTEMPTS) {
        clearInterval(timer)
        setStatus('expired')
        return
      }
      void im
        .loginWait(sessionId)
        .then((outcome) => {
          if (!outcome.ok) return
          const response = outcome.value
          setStatus(response.status)
          setMessage(response.message)
          if (response.status === 'confirmed') {
            setConfirmed(true)
            clearInterval(timer)
            toast.push('success', t('settings.im.login.confirmed'))
          }
        })
        .catch(() => {
          /* transient poll failure: the next tick retries */
        })
    }, LOGIN_POLL_INTERVAL_MS)
    return () => clearInterval(timer)
    // One poll loop per confirmed session; status transitions only stop it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, confirmed, status])

  return (
    <div className="ark-card" role="dialog" aria-label={t('settings.im.login.title')}>
      <div className="ark-card__head">
        <span className="ark-card__name">{t('settings.im.login.title')}</span>
        <div className="ark-card__actions">
          <Button size="sm" onClick={onClose}>
            {t('common.close')}
          </Button>
        </div>
      </div>
      {qrContent ? (
        <div className="ark-card__meta">
          <span>{t('settings.im.login.qrHint')}</span>
          <div className="ark-im-qr">
            {qrIsImage(qrContent) ? (
              <img src={qrContent} alt={t('settings.im.login.title')} width={QR_SIZE_PX} height={QR_SIZE_PX} />
            ) : (
              <QRCodeSVG value={qrContent} size={QR_SIZE_PX} marginSize={1} />
            )}
          </div>
        </div>
      ) : (
        <Skeleton rows={1} />
      )}
      {message ? (
        <Notice tone={status === 'failed' ? 'error' : undefined}>{message}</Notice>
      ) : null}
    </div>
  )
}
