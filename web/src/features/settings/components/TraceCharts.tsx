/**
 * Inline-SVG charts for the trace section (requirement §设置 UI + §v2).
 *
 * The stats themselves come from the backend projection — this module only
 * lays them out: a horizontal bar list for the tool distribution, one
 * keyboard-reachable button row per turn for the active-time chart, and plain
 * tables for the slow calls and the error list. No chart library, no literal
 * colours: bars derive their widths from the data and use the accent token,
 * and every string the user can read is a catalog key or a wire value.
 *
 * Clicks never stay here: every row reports its turn id through the callback
 * props and TraceSection owns the expand/scroll decision.
 */
import type { CSSProperties, ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'
import type { MessageKey } from '../../../shared/i18n'
import type { ToolStat, TraceErrorEntry, TraceStatsResponse } from '../api/trace-api'
import { formatDurationWordsMs } from './trace-format'
/** Bar cell backgrounds reuse the recessed-well token pair. */
const BAR_TRACK = 'var(--ark-bg-inset)'
const BAR_FILL = 'var(--ark-accent)'

export type TraceChartsProps = {
  stats: TraceStatsResponse
  /** Turn the timeline is currently expanded for (drives the pressed state). */
  expandedTurnId: string | null
  /** Expand (or toggle away from) one turn's timeline. */
  onToggleTurn: (turnId: string) => void
  /** Inline timeline renderer; called only for the expanded turn. */
  renderTimeline: (turnId: string) => ReactNode
}

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
 * Per-turn active time, one full-width button row per turn (§v2: the row is
 * the drill-down trigger, so it is a real `button` — keyboard focus and
 * Enter/Space come for free, unlike an SVG rect with a click handler). The
 * bar inside is a decorative gauge; the row itself carries the accessible
 * name, including the expanded state.
 */
function TurnBars({
  stats,
  expandedTurnId,
  onToggleTurn,
  renderTimeline
}: {
  stats: TraceStatsResponse
  expandedTurnId: string | null
  onToggleTurn: (turnId: string) => void
  /** Inline expansion: the panel renders under the clicked row, not at the
   * section bottom — the reader never scrolls away from the row they clicked. */
  renderTimeline: (turnId: string) => ReactNode
}): JSX.Element | null {
  const { t, locale } = useI18n()
  const turns = stats.turn_stats
  if (turns.length === 0) {
    return <p className="ark-settings__hint">{t('settings.trace.chart.turnsEmpty')}</p>
  }
  const max = Math.max(...turns.map((turn) => turn.active_ms ?? 0), 1)
  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)
  return (
    <div className="ark-trace__turns">
      {turns.map((turn, index) => {
        const activeMs = turn.active_ms ?? 0
        const ratio = Math.max(activeMs / max, activeMs > 0 ? 0.03 : 0.015)
        const expanded = expandedTurnId === turn.turn_id
        return (
          <div key={turn.turn_id} className="ark-trace__turnitem">
            <button
              type="button"
              className={`ark-trace__turnrow${expanded ? ' is-expanded' : ''}`}
              aria-expanded={expanded}
              aria-controls={`ark-trace-timeline-${turn.turn_id}`}
              onClick={() => onToggleTurn(turn.turn_id)}
            >
              <span className="ark-trace__turnrow-index">{number(index + 1)}</span>
              <span
                className={`ark-trace__turnrow-gauge${turn.failed_tool_calls > 0 ? ' is-fail' : ''}`}
                style={{ '--ark-trace-turn-ratio': `${Math.round(ratio * 100)}%` } as CSSProperties}
              />
              <span className="ark-trace__turnrow-time">
                {turn.active_ms != null ? formatDurationWordsMs(turn.active_ms, locale) : '—'}
              </span>
              <span className="ark-trace__turnrow-meta">
                {t('settings.trace.turnHint', {
                  index: index + 1,
                  duration: turn.active_ms != null ? formatDurationWordsMs(turn.active_ms, locale) : '—',
                  rounds: number(turn.model_rounds),
                  calls: number(turn.tool_calls)
                })}
              </span>
            </button>
            {expanded ? (
              <div id={`ark-trace-timeline-${turn.turn_id}`} className="ark-trace__turnpanel">
                {renderTimeline(turn.turn_id)}
              </div>
            ) : null}
          </div>
        )
      })}
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
 * `TRACE_SLOW_CALLS_LIMIT`); each row is a button that jumps to the owning
 * turn and expands its timeline.
 */
function SlowTable({
  stats,
  onToggleTurn
}: {
  stats: TraceStatsResponse
  onToggleTurn: (turnId: string) => void
}): JSX.Element | null {
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
            <tr
              key={call.call_id}
              className="is-clickable"
              onClick={() => onToggleTurn(call.turn_id)}
            >
              <td className="is-mono">{call.tool}</td>
              <td className="is-mono is-digest">{call.arguments_summary.length > 0 ? call.arguments_summary : '—'}</td>
              <td className="is-mono">{new Intl.NumberFormat(locale).format(call.duration_ms)} ms</td>
              <td className="is-action">
                {/* Keyboard path for the same jump; stopPropagation keeps the
                    row handler from toggling a second time. */}
                <button
                  type="button"
                  className="ark-linkbtn"
                  onClick={(event) => {
                    event.stopPropagation()
                    onToggleTurn(call.turn_id)
                  }}
                >
                  {t('settings.trace.timeline.locate')}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * Error list: one row per failed tool call, with the runtime errors the
 * projection correlated into that call's window rendered underneath. Rows
 * jump to their owning turn like the slow-call table does.
 */
function ErrorTable({
  errors,
  onToggleTurn
}: {
  errors: readonly TraceErrorEntry[]
  onToggleTurn: (turnId: string) => void
}): JSX.Element | null {
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
            <tr
              key={entry.call_id}
              className="is-clickable"
              onClick={() => onToggleTurn(entry.turn_id)}
            >
              <td className="is-mono">{new Date(entry.timestamp).toLocaleString(locale)}</td>
              <td className="is-mono">{entry.tool}</td>
              <td className="is-fail">
                {entry.detail ?? t('settings.trace.errorUnknown')}
                {entry.errors.map((runtime) => (
                  <span key={runtime.seq} className="ark-trace__error-runtime">
                    {t('settings.trace.errorRuntime')}: {runtime.message}
                  </span>
                ))}
                {/* Same jump as the slow table: visible button (keyboard),
                    row click as the pointer shortcut. */}
                <span className="ark-trace__error-action">
                  <button
                    type="button"
                    className="ark-linkbtn"
                    onClick={(event) => {
                      event.stopPropagation()
                      onToggleTurn(entry.turn_id)
                    }}
                  >
                    {t('settings.trace.timeline.locate')}
                  </button>
                </span>
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

/**
 * The chart area proper: distribution bars, per-turn bars, then the two
 * tables. Empty datasets degrade to hint lines — an empty section never
 * renders an empty grid.
 */
export function TraceCharts({ stats, expandedTurnId, onToggleTurn, renderTimeline }: TraceChartsProps): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-trace__charts">
      <ChartBlock titleKey="settings.trace.chart.tools">
        <ToolBars tools={stats.tool_stats} />
        <ToolTable tools={stats.tool_stats} />
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.turns">
        <TurnBars stats={stats} expandedTurnId={expandedTurnId} onToggleTurn={onToggleTurn} renderTimeline={renderTimeline} />
        <p className="ark-settings__hint">{t('settings.trace.timeline.hint')}</p>
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.slow">
        <SlowTable stats={stats} onToggleTurn={onToggleTurn} />
      </ChartBlock>
      <ChartBlock titleKey="settings.trace.chart.errors">
        <ErrorTable errors={stats.errors} onToggleTurn={onToggleTurn} />
      </ChartBlock>
    </div>
  )
}
