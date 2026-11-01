import { useMemo } from 'react'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { hasLiveItems, type ItemView } from '../state/selectors'

const STREAM_LABEL_KEYS: Record<string, MessageKey> = {
  connecting: 'canvas.footer.connecting',
  open: 'canvas.footer.open',
  reconnecting: 'canvas.footer.reconnecting',
  closed: 'canvas.footer.closed'
}

/**
 * Stream health strip under the message list. `seq` is the resume watermark:
 * showing it makes "why did I reconnect here" answerable from a screenshot.
 */
export function StreamFooter(): JSX.Element {
  const { t } = useI18n()
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastSeq = useCodeSessionStore((s) => s.lastSeq)
  // Two channels instead of the whole store: a gate or error event no longer
  // re-renders this strip.
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const live = useMemo(() => hasLiveItems({ itemOrder, itemsById } satisfies ItemView), [itemOrder, itemsById])

  return (
    <div className="ark-streamfooter">
      <span className={`ark-dot ark-dot--${streamStatus === 'open' ? 'ok' : 'warn'}`} aria-hidden="true" />
      <span className="ark-streamfooter__label">
        {STREAM_LABEL_KEYS[streamStatus] ? t(STREAM_LABEL_KEYS[streamStatus]) : streamStatus}
      </span>
      <span className="ark-streamfooter__meta">seq {lastSeq}</span>
      {live ? <span className="ark-streamfooter__meta is-live">{t('canvas.footer.generating')}</span> : null}
      {streamError ? <span className="ark-streamfooter__error">{streamError}</span> : null}
    </div>
  )
}
