/**
 * Inline-SVG charts for the trace section (requirement §设置 UI).
 *
 * The stats themselves come from the backend projection — this module only
 * lays them out: a horizontal bar list for the tool distribution, a vertical
 * bar row for per-turn active time, and plain tables for the slow calls and
 * the error list. No chart library, no literal colours: bars derive their
 * widths from the data and use the accent token, and every string the user
 * can read is a catalog key or a wire value.
 */
import type { ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n'
import type { ToolStat, TraceErrorEntry, TraceStatsResponse } from '../api/trace-api'
import { formatDurationWordsMs } from './trace-format'

/** Bar cell backgrounds reuse the recessed-well token pair. */
const BAR_TRACK = 'var(--ark-bg-inset)'
const BAR_FILL = 'var(--ark-accent)'
const BAR_FILL_WARN = 'var(--ark-error)'

/** One horizontal bar: name, proportionally filled track, count on the right. */
function HBar(props: { label: string; value: number; max: number; hint: string }): JSX.Element {
  const { label, value, max, hint } = props
  const ratio = max > 0 ? Math.max(0, Math.min(1, value / max)) : 0
  const width = Math.max(ratio * 100, value > 0 ? 1.5 : 0)
  return (
    <div className="ark-trace__hbar" title={hint}>
      <span className="ark-trace__hbar-label">{label}</span>
      <svg
        className="ark-trace__hbar-track"
        viewBox="0 0 100 8"
        preserveAspectRatio="none"
        role="img"
        aria-label={hint}
      >
        <rect x="0" y="0" width="100" height="8" rx="2" fill={BAR_TRACK} />
        <rect x="0" y="0" width={width} height="8" rx="2" fill={BAR_FILL} />
      </svg>
      <span className="ark-trace__hbar-value">{value}</span>
    </div>
  )
}

/** Tool distribution: call-count bars, per-tool failures live in the table. */
function ToolBars({ tools }: { tools: readonly ToolStat[] }): JSX.Element | null {
  const { t } = useI18n()
  if (tools.length === 0) {
    return <p className="ark-settings__hint">{t('settings.trace.chart.toolsEmpty')}</p>
  }
  const max = Math.max(...tools.map((tool) => tool.calls))
  return (
    <div className="ark-trace__bars">
      {tools.map((tool) => (
        <HBar
          key={tool.tool}
          label={tool.tool}
          value={tool.calls}
          max={max}
          hint={t('settings.trace.toolHint', { calls: tool.calls, failures: tool.failures })}
        />
      ))}
    </div>
  )
}

/**
 * Per-turn active time as vertical bars. Height is relative to the slowest
 * turn so one long turn does not flatten the rest; turns that are still
 * active (no terminal event) get the accent fill, failed turns the error
 * fill. Pure SVG, fixed viewBox, `preserveAspectRatio="none"` stretches the
 * row full-width.
 */
function TurnBars({ stats }: { stats: TraceStatsResponse }): JSX.Element | null {
  const { t, locale } = useI18n()
  const turns = stats.turn_stats
  if (turns.length === 0) {
    return <p className="ark-settings__hint">{t('settings.trace.chart.turnsEmpty')}</p>
  }
  const max = Math.max(...turns.map((turn) => turn.active_ms ?? 0), 1)
  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)
  const barWidth = 100 / turns.length
  return (
    <div>
      <svg
        className="ark-trace__vbars"
        viewBox="0 0 100 36"
        preserveAspectRatio="none"
        role="img"
        aria-label={t('settings.trace.chart.turns')}
      >
        {turns.map((turn, index) => {
          const activeMs = turn.active_ms ?? 0
          const height = Math.max((activeMs / max) * 30, activeMs > 0 ? 1 : 0.5)
          return (
            <rect
              key={turn.turn_id}
              x={index * barWidth + barWidth * 0.15}
              y={32 - height}
              width={Math.max(barWidth * 0.7, 0.5)}
              height={height}
              rx="0.8"
              fill={turn.failed_tool_calls > 0 ? BAR_FILL_WARN : BAR_FILL}
            >
              <title>
                {t('settings.trace.turnHint', {
                  index: index + 1,
                  duration: turn.active_ms != null ? formatDurationWordsMs(turn.active_ms, locale) : '—',
                  rounds: number(turn.model_rounds),
                  calls: number(turn.tool_calls)
                })}
              </title>
            </rect>
          )
        })}
      </svg>
      <p className="ark-settings__hint">
        {t('settings.trace.turnsSummary', { count: number(turns.length), total: formatDurationWordsMs(stats.active_ms ?? 0, locale) })}
      </p>
    </div>
  )
}

/** The three-column tool table: per-tool calls / failures / mean duration. */
function ToolTable({ tools }: { tools: readonly ToolStat[] }): JSX.Element | null {
  const { t, locale } = useI18n()
  if (tools.length === 0) return null
  return (
    <div className="ark-trace__table">
      <table>
        <thead>
          <tr>
            <th>{t('settings.trace.col.tool')}</th>
            <th>{t('settings.trace.col.calls')}</th>
            <th>{t('settings.trace.col.failures')}</th>
            <th>{t('settings.trace.col.avgMs')}</th>
          </tr>
        </thead>
        <tbody>
          {tools.map((tool) => (
            <tr key={tool.tool}>
              <td className="is-mono">{tool.tool}</td>
              <td>{tool.calls}</td>
              <td className={tool.failures > 0 ? 'is-fail' : ''}>{tool.failures}</td>
              <td>
                {tool.avg_duration_ms != null
                  ? `${new Intl.NumberFormat(locale).format(Math.round(tool.avg_duration_ms))} ms`
                  : '—'}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * Slow calls ranked by duration (the server already caps at
 * `TRACE_SLOW_CALLS_LIMIT`); each row carries the turn and call ids so a
 * reader can jump back into the thread timeline.
 */
function SlowTable({ stats }: { stats: TraceStatsResponse }): JSX.Element | null {
  const { t, locale } = useI18n()
  if (stats.slowest_calls.length === 0) {
    return <p className="ark-settings__hint">{t('settings.trace.chart.slowEmpty')}</p>
  }
  return (
    <div className="ark-trace__table">
      <table>
        <thead>
          <tr>
            <th>{t('settings.trace.col.tool')}</th>
            <th>{t('settings.trace.col.args')}</th>
            <th>{t('settings.trace.col.avgMs')}</th>
          </tr>
        </thead>
        <tbody>
          {stats.slowest_calls.map((call) => (
            <tr key={call.call_id}>
              <td className="is-mono">{call.tool}</td>
              <td className="is-mono is-digest">{call.arguments_summary.length > 0 ? call.arguments_summary : '—'}</td>
              <td className="is-mono">{new Intl.NumberFormat(locale).format(call.duration_ms)} ms</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * Error list: one row per failed tool call, with the runtime errors the
 * projection correlated into that call's window rendered underneath.
 */
function ErrorTable({ errors }: { errors: readonly TraceErrorEntry[] }): JSX.Element | null {
  const { t, locale } = useI18n()
  if (errors.length === 0) {
    return <p className="ark-settings__hint">{t('settings.trace.chart.errorsEmpty')}</p>
  }
  return (
    <div className="ark-trace__table">
      <table>
        <thead>
          <tr>
            <th>{t('settings.trace.col.at')}</th>
            <th>{t('settings.trace.col.tool')}</th>
            <th>{t('settings.trace.col.detail')}</th>
          </tr>
        </thead>
        <tbody>
          {errors.map((entry) => (
            <tr key={entry.call_id}>
              <td className="is-mono">{new Date(entry.timestamp).toLocaleString(locale)}</td>
              <td className="is-mono">{entry.tool}</td>
              <td className="is-fail">
                {entry.detail ?? t('settings.trace.errorUnknown')}
                {entry.errors.map((runtime) => (
                  <span key={runtime.seq} className="ark-trace__error-runtime">
                    {t('settings.trace.errorRuntime')}: {runtime.message}
                  </span>
                ))}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** One ruled sub-block with a title and its chart body. */
function ChartBlock({ titleKey, children }: { titleKey: MessageKey; children: ReactNode }): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-trace__block">
      <h4>{t(titleKey)}</h4>
      {children}
    </div>
  )
}

export type TraceChartsProps = { stats: TraceStatsResponse }

/**
 * The chart area proper: distribution bars, per-turn bars, then the two
 * tables. Empty datasets degrade to hint lines — an empty section never
 * renders an empty grid.
 */
export function TraceCharts({ stats }: TraceChartsProps): JSX.Element {
  return (
    <div className="ark-trace__charts">
      <ChartBlock titleKey="settings.trace.chart.tools">
        <ToolBars tools={stats.tool_stats} />
        <ToolTable tools={stats.tool_stats} />
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.turns">
        <TurnBars stats={stats} />
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.slow">
        <SlowTable stats={stats} />
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.errors">
        <ErrorTable errors={stats.errors} />
      </ChartBlock>
    </div>
  )
}
