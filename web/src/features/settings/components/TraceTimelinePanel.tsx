/**
 * Single-turn timeline panel (requirement trace-analysis.md §v2).
 *
 * Layout mirrors the smart-ark turn Gantt the feature was ported from: a
 * fixed label column (disclosure arrow, kind chip, label), a shared-axis bar
 * lane, and a fixed duration column. Rows expand in place to a detail block
 * (type/start/duration, plus nested reasoning/assistant text on model calls).
 * The panel fetches only when shown; every colour, position and error mark
 * projects the wire payload — the server stays the only place that folds data.
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
import {
  fetchTraceTimeline,
  type TraceSpanDetail,
  type TraceSpanDetailKind,
  type TraceSpanUsage,
  type TraceTimeline,
  type TraceTimelineSpan
} from '../api/trace-api'
import { Skeleton } from './SectionShell'
import { formatDurationWordsMs } from './trace-format'

export type TraceTimelinePanelProps = {
  threadId: string
  turnId: string
}

/** smart-ark palette: one stable colour per span kind, so a bar is readable
 * before its label is. Local CSS variables instead of hex in JSX keep the
 * theme seams (error colour) in one place. */
const SPAN_COLOR: Record<TraceTimelineSpan['kind'], string> = {
  user_message: 'var(--ark-gantt-user)',
  assistant_text: 'var(--ark-gantt-assistant)',
  reasoning: 'var(--ark-gantt-reasoning)',
  tool_call: 'var(--ark-gantt-tool)',
  approval: 'var(--ark-gantt-approval)',
  user_input: 'var(--ark-gantt-input)',
  compaction: 'var(--ark-gantt-compaction)',
  model_call: 'var(--ark-gantt-model)',
  error: 'var(--ark-gantt-error)',
  notice: 'var(--ark-gantt-notice)',
}

/**
 * The shared time domain: first span start → latest known end. A span's end
 * is its start plus the measured duration; text-only spans (no duration)
 * participate through their start only.
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
 * Ruler labels: the reference panel shows 0 / ¼ / ½ / ¾ / total as relative
 * compact durations over the shared axis.
 */
function rulerTicks(start: number, end: number, locale: ReturnType<typeof useI18n>['locale']): string[] {
  const span = end - start
  return [0, 0.25, 0.5, 0.75, 1].map((fraction) =>
    formatDurationWordsMs(Math.round(span * fraction), locale)
  )
}

/** Detail kind -> localized catalog key. The wire keeps the snake_case value;
 * translation stays in the catalog so both languages share one vocabulary. */
const DETAIL_KIND_KEY: Record<TraceSpanDetailKind, `settings.trace.span.kind.${TraceSpanDetailKind}`> = {
  reasoning: 'settings.trace.span.kind.reasoning',
  assistant_text: 'settings.trace.span.kind.assistant_text'
}

/**
 * One expanded detail fragment: localized kind, its own start time, then the
 * bounded snippet as monospace pre-wrapped text (multi-line body stays intact).
 * Order is the server's: reasoning/assistant text interleave by `started_at`.
 */
function DetailRow({ detail, locale }: { detail: TraceSpanDetail; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  return (
    <li className="ark-gantt__detail-row">
      <span className="ark-gantt__detail-meta">
        <b>{t(DETAIL_KIND_KEY[detail.kind])}</b>
        <time dateTime={detail.started_at}>{formatClock(detail.started_at, locale)}</time>
      </span>
      <pre className="ark-gantt__detail-text">{detail.text_snippet}</pre>
    </li>
  )
}

/** Shared detail section shell: all metric blocks and text fragments use it. */
function DetailSection({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <div className="ark-gantt__detail-section">
      <span className="ark-gantt__detail-title">{title}</span>
      {children}
    </div>
  )
}

function formatCount(value: number, locale: ReturnType<typeof useI18n>['locale']): string {
  return new Intl.NumberFormat(locale).format(value)
}

/**
 * Mutually exclusive token segments, mirroring smart-ark's
 * `buildTokenSegments`: `cached` is a subset of `prompt` (OpenAI counts
 * cache hits inside prompt_tokens; Anthropic decomposes prompt into input +
 * cache_read + cache_creation), so input-total and cache-hit must never be
 * presented side by side as if they were disjoint. Splitting into
 * uncached-input / cache-hit / output makes the shares sum to `total` and
 * every percentage honest. Only arithmetic the wire does not carry; no
 * provider value is invented when the cache counters are absent.
 */
function tokenSegments(usage: TraceSpanUsage): Array<{ key: 'uncached' | 'hit' | 'output'; labelKey: 'settings.trace.gantt.tokenUncached' | 'settings.trace.gantt.tokenCached' | 'settings.trace.gantt.tokenOutput'; value: number }> {
  const prompt = Math.max(0, usage.prompt_tokens)
  const completion = Math.max(0, usage.completion_tokens)
  const cached = usage.cache_hit_tokens != null
    ? Math.max(0, Math.min(usage.cache_hit_tokens, prompt))
    : 0
  const uncached = Math.max(0, prompt - cached)
  const segments: Array<{ key: 'uncached' | 'hit' | 'output'; labelKey: 'settings.trace.gantt.tokenUncached' | 'settings.trace.gantt.tokenCached' | 'settings.trace.gantt.tokenOutput'; value: number }> = [
    { key: 'uncached', labelKey: 'settings.trace.gantt.tokenUncached', value: uncached },
    { key: 'hit', labelKey: 'settings.trace.gantt.tokenCached', value: cached },
    { key: 'output', labelKey: 'settings.trace.gantt.tokenOutput', value: completion },
  ]
  return segments.filter((segment) => segment.value > 0)
}

/**
 * Token composition bar + per-segment counts, following smart-ark's
 * `buildTokenChartHtml`. The total row stays visible so the shares have a
 * denominator; a provider that omits cache counters simply renders the
 * uncached-input and output segments, without claiming a cache split.
 */
function TokenMetrics({ usage, locale }: { usage: TraceSpanUsage; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  const segments = tokenSegments(usage)
  const denominator = Math.max(1, usage.total_tokens)
  return (
    <DetailSection title={t('settings.trace.gantt.tokenTitle')}>
      <div className="ark-gantt__token-total">
        {t('settings.trace.gantt.tokenTotal')}: <b>{formatCount(usage.total_tokens, locale)}</b>
      </div>
      <div className="ark-gantt__token-bar" role="img" aria-label={t('settings.trace.gantt.tokenTitle')}>
        {segments.map((segment) => (
          <span
            key={segment.key}
            className={`ark-gantt__token-segment is-${segment.key}`}
            style={{ width: `${(segment.value / denominator) * 100}%` }}
          />
        ))}
      </div>
      <div className="ark-gantt__token-legend">
        {segments.map((segment) => (
          <span key={segment.key} className="ark-gantt__token-legend-item">
            <i className={`is-${segment.key}`} aria-hidden="true" />
            {t(segment.labelKey)}: <b>{formatCount(segment.value, locale)}</b>
            <em>{new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 }).format(segment.value / denominator)}</em>
          </span>
        ))}
      </div>
    </DetailSection>
  )
}

/**
 * Cache summary: the hit rate already comes from the provider (or its
 * documented derivation) — the renderer only formats it. All fields absent
 * means "not reported", never a synthesized zero.
 */
function CacheMetrics({ usage, locale }: { usage: TraceSpanUsage; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  const hasHit = usage.cache_hit_tokens != null
  const hasMiss = usage.cache_miss_tokens != null
  const hasRate = usage.cache_hit_rate != null
  if (!hasHit && !hasMiss && !hasRate) {
    return (
      <DetailSection title={t('settings.trace.gantt.cacheTitle')}>
        <span className="ark-gantt__detail-empty">{t('settings.trace.gantt.cacheUnreported')}</span>
      </DetailSection>
    )
  }
  return (
    <DetailSection title={t('settings.trace.gantt.cacheTitle')}>
      <div className="ark-gantt__detail-metrics">
        {hasHit ? <span>{t('settings.trace.gantt.cacheHit')}: <b>{formatCount(usage.cache_hit_tokens!, locale)}</b></span> : null}
        {hasMiss ? <span>{t('settings.trace.gantt.cacheMiss')}: <b>{formatCount(usage.cache_miss_tokens!, locale)}</b></span> : null}
        {hasRate ? (
          <span>
            {t('settings.trace.gantt.cacheRate')}:{' '}
            <b>
              {usage.cache_hit_rate_approximate === true ? `${t('settings.trace.gantt.cacheRateApprox')} ` : ''}
              {new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 }).format(usage.cache_hit_rate!)}
            </b>
          </span>
        ) : null}
      </div>
    </DetailSection>
  )
}

/**
 * The expanded body of one span. On a model call the streamed text fragments
 * follow the meta line, separated by a hairline; other spans keep the plain
 * type/start/duration block. Details are never rendered as independent rows —
 * the server only nests them where a model call actually produced text.
 */
function SpanDetails({ span, locale, failed }: {
  span: TraceTimelineSpan
  locale: ReturnType<typeof useI18n>['locale']
  failed: boolean
}): JSX.Element {
  const { t } = useI18n()
  const hasDuration = span.duration_ms != null
  const details = span.details
  return (
    <div className="ark-gantt__detail">
      <span>
        {t('settings.trace.gantt.kind')}:{' '}
        <b>{t(`settings.trace.span.kind.${span.kind}`)}</b>
      </span>
      <span>
        {t('settings.trace.gantt.started')}: <b>{formatClock(span.started_at, locale)}</b>
      </span>
      {hasDuration ? (
        <span>
          {t('settings.trace.gantt.duration')}:{' '}
          <b>{formatDurationWordsMs(span.duration_ms ?? 0, locale)}</b>
        </span>
      ) : null}
      {failed ? <span className="ark-gantt__detail-failed">{t('settings.trace.span.failed')}</span> : null}
      {span.kind === 'model_call' && span.usage != null ? (
        <>
          <TokenMetrics usage={span.usage} locale={locale} />
          <CacheMetrics usage={span.usage} locale={locale} />
        </>
      ) : null}
      {details ? (
        <DetailSection title={t('settings.trace.gantt.detailsTitle')}>
          {details.length > 0 ? (
            <ul className="ark-gantt__detail-rows">
              {details.map((detail, index) => (
                <DetailRow key={`${detail.kind}-${detail.started_at}-${index}`} detail={detail} locale={locale} />
              ))}
            </ul>
          ) : (
            <span className="ark-gantt__detail-empty">{t('settings.trace.gantt.detailsEmpty')}</span>
          )}
        </DetailSection>
      ) : null}
    </div>
  )
}

/**
 * One Gantt row in the reference layout: label column (disclosure arrow,
 * kind chip, label), the bar lane on the shared axis, and the duration
 * column. A row without a measured duration shows `—` in the duration column
 * and renders a minimum-width tick in the lane — visible, never a fake
 * duration.
 */
function SpanRow({
  span,
  window: axis,
  locale,
  expanded,
  onToggle
}: {
  span: TraceTimelineSpan
  window: { start: number; end: number }
  locale: ReturnType<typeof useI18n>['locale']
  expanded: boolean
  onToggle: () => void
}): JSX.Element {
  const { t } = useI18n()
  const failed = span.is_error === true
  const at = Date.parse(span.started_at)
  const leftPct = Math.min(100, Math.max(0, ((at - axis.start) / (axis.end - axis.start)) * 100))
  const hasDuration = span.duration_ms != null
  const widthPct = hasDuration
    ? Math.max(0.5, ((span.duration_ms ?? 0) / (axis.end - axis.start)) * 100)
    : null
  const timing = hasDuration
    ? `${formatClock(span.started_at, locale)} · ${formatDurationWordsMs(span.duration_ms ?? 0, locale)}`
    : formatClock(span.started_at, locale)
  return (
    <li className="ark-gantt__item">
      <button
        type="button"
        className={`ark-gantt__row${failed ? ' is-failed' : ''}`}
        data-span-kind={span.kind}
        aria-expanded={expanded}
        onClick={onToggle}
        title={`${t(`settings.trace.span.kind.${span.kind}`)}${span.label ? ` · ${span.label}` : ''} · ${timing}`}
      >
        <span className="ark-gantt__label">
          <span className="ark-gantt__chevron" aria-hidden="true">
            {expanded ? '▾' : '▸'}
          </span>
          <span
            className="ark-gantt__chip"
            aria-hidden="true"
            style={{ '--ark-span-color': SPAN_COLOR[span.kind] } as CSSProperties}
          />
          <span className="ark-gantt__name">
            {t(`settings.trace.span.kind.${span.kind}`)}
            {span.label ? <span className="ark-gantt__sub">{span.label}</span> : null}
          </span>
        </span>
        <span className="ark-gantt__lane" aria-hidden="true">
          <span
            className={`ark-gantt__bar${failed ? ' is-failed' : ''}`}
            style={{
              '--ark-span-left': `${leftPct}%`,
              '--ark-span-width': widthPct != null ? `${widthPct}%` : undefined,
              '--ark-span-color': SPAN_COLOR[span.kind]
            } as CSSProperties}
          />
        </span>
        <span className="ark-gantt__duration">
          {hasDuration ? formatDurationWordsMs(span.duration_ms ?? 0, locale) : '—'}
        </span>
      </button>
      {expanded ? <SpanDetails span={span} locale={locale} failed={failed} /> : null}
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
  const [expandedKeys, setExpandedKeys] = useState<Set<string>>(new Set())

  useEffect(() => {
    const controller = new AbortController()
    setStatus('loading')
    setError(null)
    setTimeline(null)
    setExpandedKeys(new Set())
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

  const axis = timelineWindow(timeline.spans)
  const ticks = rulerTicks(axis.start, axis.end, locale)
  const toggle = (key: string): void => {
    setExpandedKeys((previous) => {
      const next = new Set(previous)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  return (
    <div className="ark-timeline">
      <div className="ark-gantt__summary">
        {t('settings.trace.gantt.summary', {
          count: timeline.spans.length,
          total: formatDurationWordsMs(axis.end - axis.start, locale)
        })}
      </div>
      <div className="ark-gantt__ruler" aria-hidden="true">
        <span className="ark-gantt__ruler-label" />
        <div className="ark-gantt__ruler-scale">
          {ticks.map((label, index) => (
            <span key={`${label}-${index}`}>{label}</span>
          ))}
        </div>
        <span className="ark-gantt__ruler-duration" />
      </div>
      <ul className="ark-gantt__list">
        {timeline.spans.map((span, index) => {
          const key = `${index}-${span.item_id ?? span.started_at}`
          return (
            <SpanRow
              key={key}
              span={span}
              window={axis}
              locale={locale}
              expanded={expandedKeys.has(key)}
              onToggle={() => toggle(key)}
            />
          )
        })}
      </ul>
      <p className="ark-settings__hint">{t('settings.trace.timeline.ganttHint')}</p>
    </div>
  )
}
