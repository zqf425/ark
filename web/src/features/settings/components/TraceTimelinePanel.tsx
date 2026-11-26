/**
 * Single-turn timeline panel (requirement trace-analysis.md §v2).
 *
 * The panel fetches only when it is actually shown — mount alone never starts
 * a request, and a re-expand re-requests rather than keeping a second copy of
 * the backend projection. Spans render as a shared-axis Gantt: every span is
 * one bar positioned by its start fraction and sized by its duration share;
 * spans without a measured duration (text rows) render as tick marks so the
 * whole turn still reads as one time domain. Every colour, position and error
 * mark is a projection of the wire payload, never recomputed here.
 */
import { useEffect, useState } from 'react'
import type { CSSProperties } from 'react'
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
 * The shared time domain: first span start → latest known end. A span's end
 * is its start plus the measured duration; text-only spans (no duration)
 * participate through their start only. Pure display arithmetic over the wire
 * payload — the server stays the only place that folds event data.
 */
function timelineWindow(spans: readonly TraceTimelineSpan[]): { start: number; end: number } {
  const starts = spans.map((span) => Date.parse(span.started_at))
  const start = Math.min(...starts)
  const ends = spans.map((span) => {
    const at = Date.parse(span.started_at)
    return span.duration_ms != null ? at + span.duration_ms : at
  })
  const end = Math.max(...ends, start)
  // `max` guards the degenerate all-instant turn so widths stay finite.
  return { start, end: Math.max(end, start + 1) }
}

/**
 * One Gantt row: the kind chip plus a bar positioned on the shared axis. Rows
 * with a measured duration get a proportional bar; rows without one (text
 * spans) get a fixed-width tick at their start — visible, but never reading
 * as a fake duration. The tooltip carries kind + label + exact timing, so the
 * compressed bar never hides the numbers.
 */
function SpanRow({ span, window: axis, locale }: { span: TraceTimelineSpan; window: { start: number; end: number }; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  const failed = span.is_error === true
  const at = Date.parse(span.started_at)
  const leftPct = Math.min(100, Math.max(0, ((at - axis.start) / (axis.end - axis.start)) * 100))
  const hasDuration = span.duration_ms != null
  const widthPct = hasDuration
    ? Math.max(0.75, ((span.duration_ms ?? 0) / (axis.end - axis.start)) * 100)
    : null
  const timing = hasDuration
    ? `${formatClock(span.started_at, locale)} · ${formatDurationWordsMs(span.duration_ms ?? 0, locale)}`
    : formatClock(span.started_at, locale)
  return (
    <li
      className={`ark-timeline__row${failed ? ' is-failed' : ''}`}
      data-span-kind={span.kind}
      title={`${t(`settings.trace.span.kind.${span.kind}`)}${span.label ? ` · ${span.label}` : ''} · ${timing}`}
    >
      <span className="ark-timeline__chip" aria-hidden="true" />
      <span className="ark-timeline__lane" aria-hidden="true">
        <span
          className={`ark-timeline__bar${hasDuration ? '' : ' is-tick'}`}
          style={{ '--ark-span-left': `${leftPct}%`, '--ark-span-width': widthPct != null ? `${widthPct}%` : undefined } as CSSProperties}
        />
      </span>
      <span className="ark-timeline__kind">{t(`settings.trace.span.kind.${span.kind}`)}</span>
      {span.label ? <span className="ark-timeline__label">{span.label}</span> : null}
      {failed ? <span className="ark-timeline__failed">{t('settings.trace.span.failed')}</span> : null}
      {span.text_snippet ? <span className="ark-timeline__snippet">{span.text_snippet}</span> : null}
      <span className="ark-timeline__when">{timing}</span>
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
      <p className="ark-settings__hint">{t('settings.trace.timeline.ganttHint')}</p>
      <ul className="ark-timeline__list">
        {(() => {
          const axis = timelineWindow(timeline.spans)
          return timeline.spans.map((span, index) => (
            <SpanRow key={`${index}-${span.item_id ?? span.started_at}`} span={span} window={axis} locale={locale} />
          ))
        })()}
      </ul>
    </div>
  )
}
