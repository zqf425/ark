/**
 * Single-turn timeline panel (requirement trace-analysis.md §v2).
 *
 * The panel fetches only when it is actually shown — mount alone never starts
 * a request, and a re-expand re-requests rather than keeping a second copy of
 * the backend projection. Spans render top-down in the order the server
 * produced them; every colour, duration and error mark is a projection of the
 * wire payload, never recomputed here.
 */
import { useEffect, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { EmptyState } from '../../../shared/ui'
import type { LoadStatus } from '../../../shared/contracts/web-only'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { useMounted } from '../../../shared/api/use-mounted'
import { formatClock } from '../../../shared/format'
import { fetchTraceTimeline, type TraceTimeline, type TraceTimelineSpan } from '../api/trace-api'
import { Skeleton } from './SectionShell'
import { formatDurationWordsMs } from './trace-format'

export type TraceTimelinePanelProps = {
  threadId: string
  turnId: string
}

/**
 * One vertical span row: a kind-coloured chip, the kind's localized label plus
 * the label field, and the real duration only when the server reported one —
 * a text span without `duration_ms` stays silent instead of reading "0ms".
 */
function SpanRow({ span, locale }: { span: TraceTimelineSpan; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  const failed = span.is_error === true
  return (
    <li className={`ark-timeline__row${failed ? ' is-failed' : ''}`} data-span-kind={span.kind}>
      <span className="ark-timeline__chip" aria-hidden="true" />
      <span className="ark-timeline__kind">{t(`settings.trace.span.kind.${span.kind}`)}</span>
      {span.label ? <span className="ark-timeline__label">{span.label}</span> : null}
      {failed ? <span className="ark-timeline__failed">{t('settings.trace.span.failed')}</span> : null}
      {span.text_snippet ? <span className="ark-timeline__snippet">{span.text_snippet}</span> : null}
      <span className="ark-timeline__when">{formatClock(span.started_at, locale)}</span>
      {span.duration_ms != null ? (
        <span className="ark-timeline__duration">{formatDurationWordsMs(span.duration_ms, locale)}</span>
      ) : null}
    </li>
  )
}

/**
 * The expanded body of one turn. `expandedTurnId` switching in TraceSection
 * swaps which instance is mounted, so the fetch effect runs per activation.
 */
export function TraceTimelinePanel({ threadId, turnId }: TraceTimelinePanelProps): JSX.Element {
  const { t, locale } = useI18n()
  const mounted = useMounted()
  const [timeline, setTimeline] = useState<TraceTimeline | null>(null)
  const [status, setStatus] = useState<LoadStatus>('loading')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setStatus('loading')
    setError(null)
    setTimeline(null)
    fetchTraceTimeline(threadId, turnId, controller.signal)
      .then((response) => {
        if (!mounted.current || controller.signal.aborted) return
        setTimeline(response)
        setStatus('ready')
      })
      .catch((reason: unknown) => {
        if (!mounted.current || controller.signal.aborted) return
        setError(describeError(reason, translateNow))
        setStatus('error')
      })
    return () => controller.abort()
  }, [threadId, turnId, mounted])

  if (status === 'loading') {
    return (
      <div className="ark-timeline" role="status">
        <Skeleton rows={2} />
        <p className="ark-settings__hint">{t('settings.trace.timeline.loading')}</p>
      </div>
    )
  }

  if (status === 'error') {
    return (
      <div className="ark-timeline" role="alert">
        <p className="ark-settings__hint ark-settings__hint--error">
          {t('settings.trace.timeline.loadFailed')}
          {error ? `：${error}` : ''}
        </p>
      </div>
    )
  }

  // A 404 (turn predates the timeline window) and a genuinely empty turn both
  // land here: the panel degrades to the empty state, not an error.
  if (!timeline || timeline.spans.length === 0) {
    return (
      <div className="ark-timeline">
        <EmptyState title={t('settings.trace.timeline.empty')} hint={t('settings.trace.timeline.emptyHint')} />
      </div>
    )
  }

  return (
    <div className="ark-timeline">
      <ul className="ark-timeline__list">
        {timeline.spans.map((span, index) => (
          <SpanRow key={`${index}-${span.item_id ?? span.started_at}`} span={span} locale={locale} />
        ))}
      </ul>
    </div>
  )
}
