/**
 * Session-level token composition for the trace analysis section.
 *
 * The response's `usage` is already the backend's session-wide `Usage::merge`
 * result: this component only formats it and splits it into mutually
 * exclusive shares (non-cached input / cache miss / cache hit / output). No second
 * fold, no provider counter reconstructed from a missing field.
 */
import { useI18n } from '../../../shared/i18n'
import type { Usage } from '../../../shared/contracts'
import { traceCacheHitRate, traceTokenCacheReported, traceTokenSegments, type TraceTokenSegment } from '../api/trace-api'

type Locale = ReturnType<typeof useI18n>['locale']

function formatCount(value: number, locale: Locale): string {
  return new Intl.NumberFormat(locale).format(value)
}

function formatShare(value: number, total: number, locale: Locale): string {
  return new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 }).format(value / total)
}

/**
 * A legend item pairs the localized name with the exact count and the share
 * of the session total. The swatch repeats the bar colour so the two views
 * read as one chart.
 */
function LegendItem({ segment, total, locale }: {
  segment: TraceTokenSegment
  total: number
  locale: Locale
}): JSX.Element {
  const { t } = useI18n()
  return (
    <span className="ark-trace__token-legend-item">
      <i className={`is-${segment.key}`} aria-hidden="true" />
      {t(segment.labelKey)}: <b>{formatCount(segment.value, locale)}</b>
      <em>{formatShare(segment.value, total, locale)}</em>
    </span>
  )
}

/**
 * Whole-session token readout between the metric cards and the charts.
 * Up to four disjoint shares add up to the authoritative total; when no cache
 * counter was reported, only non-cached input and output render and an
 * explicit hint replaces the cache claim.
 */
export function SessionTokenOverview({ usage, locale }: { usage: Usage | undefined; locale: Locale }): JSX.Element {
  const { t } = useI18n()
  const segments = traceTokenSegments(usage)
  // `total_tokens` is authoritative even when it disagrees with the prompt /
  // completion split; the bar width may then sum to less than 100%. The
  // legend uses the same denominator so no share is silently renormalized.
  const total = Math.max(0, usage?.total_tokens ?? 0)
  const cacheReported = traceTokenCacheReported(usage)
  // The header's convention: hit/(hit+miss), independent of the sampled wire
  // rate. Approximate is still the backend's label for a derived miss count.
  const rate = traceCacheHitRate(usage)
  const percentFormat = new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 })
  const percent = (value: number): string => percentFormat.format(value / Math.max(1, total))

  return (
    <div className="ark-trace__token-overview">
      <h3>{t('settings.trace.token.title')}</h3>
      <div className="ark-trace__token-summary">
        <span>
          {t('settings.trace.gantt.tokenTotal')}: <b>{formatCount(total, locale)}</b>
        </span>
        {rate != null ? (
          <span title={t('settings.trace.gantt.cacheRateHint')}>
            {t('settings.trace.gantt.cacheRate')}:{' '}
            <b>
              {usage?.cache_hit_rate_approximate === true ? `${t('settings.trace.gantt.cacheRateApprox')} ` : ''}
              {percentFormat.format(rate)}
            </b>
          </span>
        ) : null}
      </div>
      {total > 0 ? (
        <div
          className="ark-trace__token-bar"
          role="img"
          aria-label={t('settings.trace.token.title')}
        >
          {segments.map((segment) => (
            <span
              key={segment.key}
              className={`ark-trace__token-segment is-${segment.key}`}
              style={{ width: percent(segment.value) }}
              title={`${t(segment.labelKey)} · ${formatShare(segment.value, total, locale)}`}
            />
          ))}
        </div>
      ) : null}
      <div className="ark-trace__token-legend">
        {segments.map((segment) => (
          <LegendItem key={segment.key} segment={segment} total={total} locale={locale} />
        ))}
        <span className="ark-trace__token-share-note">{t('settings.trace.token.shareOfTotal')}</span>
      </div>
      {!cacheReported ? (
        <span className="ark-trace__token-unreported">{t('settings.trace.gantt.cacheUnreported')}</span>
      ) : null}
    </div>
  )
}
