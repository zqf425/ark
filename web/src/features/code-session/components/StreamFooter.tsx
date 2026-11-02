import { useMemo } from 'react'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { formatTokens, formatUsd } from '../../../shared/format'
import { useCodeSessionStore } from '../state/store'
import { hasLiveItems, type ItemView } from '../state/selectors'
import { useContextLimit } from '../hooks/use-context-limit'

const STREAM_LABEL_KEYS: Record<string, MessageKey> = {
  connecting: 'canvas.footer.connecting',
  open: 'canvas.footer.open',
  reconnecting: 'canvas.footer.reconnecting',
  closed: 'canvas.footer.closed'
}

/** Context usage past this fraction of the limit renders in the warning tone. */
const CONTEXT_WARN_RATIO = 0.8

/**
 * Stream health strip under the message list. `seq` is the resume watermark:
 * showing it makes "why did I reconnect here" answerable from a screenshot.
 * Usage and the newest compaction ride along so token spend and context
 * pressure are visible without leaving the canvas.
 */
export function StreamFooter(): JSX.Element {
  const { t, locale } = useI18n()
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastSeq = useCodeSessionStore((s) => s.lastSeq)
  const usage = useCodeSessionStore((s) => s.usage)
  const lastCompaction = useCodeSessionStore((s) => s.lastCompaction)
  // Two channels instead of the whole store: a gate or error event no longer
  // re-renders this strip.
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const live = useMemo(() => hasLiveItems({ itemOrder, itemsById } satisfies ItemView), [itemOrder, itemsById])
  const contextLimit = useContextLimit()

  const contextRatio = usage && contextLimit && contextLimit > 0 && usage.lastPromptTokens != null
    ? usage.lastPromptTokens / contextLimit
    : null

  return (
    <div className="ark-streamfooter">
      <span className={`ark-dot ark-dot--${streamStatus === 'open' ? 'ok' : 'warn'}`} aria-hidden="true" />
      <span className="ark-streamfooter__label">
        {STREAM_LABEL_KEYS[streamStatus] ? t(STREAM_LABEL_KEYS[streamStatus]) : streamStatus}
      </span>
      <span className="ark-streamfooter__meta">seq {lastSeq}</span>
      {usage ? (
        <span className="ark-streamfooter__meta" title={t('canvas.usage.title')}>
          {t('canvas.usage.tokens', {
            prompt: formatTokens(usage.promptTokens),
            completion: formatTokens(usage.completionTokens)
          })}
          {usage.costUsd != null ? ` · ${formatUsd(usage.costUsd, locale)}` : ''}
        </span>
      ) : null}
      {contextRatio != null ? (
        <span
          className={`ark-streamfooter__meta${contextRatio >= CONTEXT_WARN_RATIO ? ' is-warning' : ''}`}
          title={t('canvas.usage.contextTitle')}
        >
          {t('canvas.usage.context', {
            used: formatTokens(usage?.lastPromptTokens ?? 0),
            limit: formatTokens(contextLimit ?? 0),
            percent: Math.round(contextRatio * 100)
          })}
        </span>
      ) : null}
      {lastCompaction ? (
        <span className="ark-streamfooter__meta" title={t('canvas.compaction.title')}>
          {t('canvas.compaction.notice', {
            removed: lastCompaction.removedItems,
            chars: formatTokens(lastCompaction.checkpointChars)
          })}
        </span>
      ) : null}
      {live ? <span className="ark-streamfooter__meta is-live">{t('canvas.footer.generating')}</span> : null}
      {streamError ? <span className="ark-streamfooter__error">{streamError}</span> : null}
    </div>
  )
}
