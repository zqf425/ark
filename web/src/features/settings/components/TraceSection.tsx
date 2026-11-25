/**
 * Trace analysis section (requirements/trace-analysis.md).
 *
 * A read-only statistics view over `GET /v1/threads/{id}/trace-stats`: pick a
 * session, read the metric cards and inline-SVG charts, and — only on an
 * explicit button press — create the analysis session. The default state
 * never creates a session and never calls the model: the two POSTs live
 * exclusively behind `startAnalysis`, and the button stays disabled until the
 * stats have loaded.
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ThreadSummary } from '../../../shared/contracts'
import type { LoadStatus } from '../../../shared/contracts/web-only'
import {
  analysisThreadTitle,
  buildAnalysisPrompt,
  createAnalysisThread,
  fetchTraceStats,
  listThreads,
  startAnalysisTurn,
  traceTokenTotals,
  type TraceStatsResponse
} from '../api/trace-api'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { useMounted } from '../../../shared/api/use-mounted'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'
import { TraceCharts } from './TraceCharts'
import { formatDurationWordsMs } from './trace-format'

/** One metric card: label above, value below (provider-stats row pattern). */
function MetricCard({ label, value }: { label: string; value: string }): JSX.Element {
  return (
    <div className="ark-trace__metric">
      <span>{label}</span>
      <b>{value}</b>
    </div>
  )
}

/** The headline counters above the charts (requirement §设置 UI). */
function MetricRow({ stats, locale }: { stats: TraceStatsResponse; locale: ReturnType<typeof useI18n>['locale'] }): JSX.Element {
  const { t } = useI18n()
  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)
  const tokens = traceTokenTotals(stats.usage)
  return (
    <div className="ark-trace__metrics">
      <MetricCard label={t('settings.trace.metrics.turns')} value={number(stats.turns)} />
      <MetricCard label={t('settings.trace.metrics.llmCalls')} value={number(stats.model_calls)} />
      <MetricCard label={t('settings.trace.metrics.toolCalls')} value={number(stats.tool_calls)} />
      <MetricCard label={t('settings.trace.metrics.toolFailures')} value={number(stats.failed_tool_calls)} />
      <MetricCard
        label={t('settings.trace.metrics.activeMs')}
        value={stats.active_ms != null ? formatDurationWordsMs(stats.active_ms, locale) : '—'}
      />
      <MetricCard label={t('settings.trace.metrics.tokens')} value={number(tokens.total_tokens)} />
      <MetricCard label={t('settings.trace.metrics.activeDays')} value={number(stats.active_days)} />
    </div>
  )
}

/** Picker rows are plain threads, sorted by the server order already applied. */
function ThreadPicker(props: {
  threads: readonly ThreadSummary[]
  selectedId: string | null
  onSelect: (threadId: string) => void
  loading: boolean
}): JSX.Element {
  const { t, locale } = useI18n()
  const { threads, selectedId, onSelect, loading } = props
  const formatWhen = (iso: string): string => {
    const time = Date.parse(iso)
    return Number.isNaN(time) ? '—' : new Date(time).toLocaleString(locale)
  }
  return (
    <label className="ark-settings__hint ark-trace__picker">
      {t('settings.trace.threadLabel')}
      <select
        className="ark-field__select"
        value={selectedId ?? ''}
        disabled={loading}
        onChange={(event) => onSelect(event.target.value)}
      >
        <option value="" disabled>
          {t('settings.trace.threadPlaceholder')}
        </option>
        {threads.map((thread) => (
          <option key={thread.thread_id} value={thread.thread_id}>
            {thread.title || t('canvas.untitled')} · {formatWhen(thread.last_event_at ?? thread.updated_at)}
          </option>
        ))}
      </select>
    </label>
  )
}

export function TraceSection(): JSX.Element {
  const { t, locale } = useI18n()
  const toast = useToast()
  const mounted = useMounted()

  const [threads, setThreads] = useState<ThreadSummary[]>([])
  const [threadsStatus, setThreadsStatus] = useState<LoadStatus>('idle')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [stats, setStats] = useState<TraceStatsResponse | null>(null)
  const [statsStatus, setStatsStatus] = useState<LoadStatus>('idle')
  const [statsError, setStatsError] = useState<string | null>(null)
  const [starting, setStarting] = useState(false)

  // One list load per mount; the picker never re-fetches on selection.
  useEffect(() => {
    const controller = new AbortController()
    setThreadsStatus('loading')
    listThreads(controller.signal)
      .then((rows) => {
        if (!mounted.current) return
        setThreads(rows)
        setThreadsStatus('ready')
      })
      .catch(() => {
        // A failed list keeps the section usable: stats stay reachable by
        // retrying, and an empty picker reads as "nothing to analyze".
        if (mounted.current) setThreadsStatus('error')
      })
    return () => controller.abort()
  }, [mounted])

  // Stats follow the selection; a stale response is dropped by the mounted
  // guard plus the selection check (last selection wins).
  useEffect(() => {
    if (!selectedId) return
    const controller = new AbortController()
    setStatsStatus('loading')
    setStatsError(null)
    fetchTraceStats(selectedId, controller.signal)
      .then((response) => {
        if (!mounted.current) return
        setStats(response)
        setStatsStatus('ready')
      })
      .catch((reason: unknown) => {
        if (!mounted.current || controller.signal.aborted) return
        setStats(null)
        setStatsError(describeError(reason, translateNow))
        setStatsStatus('error')
      })
    return () => controller.abort()
  }, [selectedId, mounted])

  const reload = useCallback((): void => {
    if (!selectedId) return
    // Re-running the projection is the same fetch the selection effect runs;
    // a fresh key re-enters it without a second code path.
    setStatsStatus('loading')
    fetchTraceStats(selectedId)
      .then((response) => {
        if (!mounted.current) return
        setStats(response)
        setStatsError(null)
        setStatsStatus('ready')
      })
      .catch((reason: unknown) => {
        if (!mounted.current) return
        setStatsError(describeError(reason, translateNow))
        setStatsStatus('error')
      })
  }, [mounted, selectedId])

  const selectedThread = useMemo(
    () => threads.find((thread) => thread.thread_id === selectedId) ?? null,
    [threads, selectedId]
  )

  // The only path to the two POSTs. Everything above is read-only.
  const startAnalysis = async (): Promise<void> => {
    if (!stats || !selectedThread || starting) return
    setStarting(true)
    try {
      const created = await createAnalysisThread(analysisThreadTitle(selectedThread.title))
      await startAnalysisTurn(created.thread_id, buildAnalysisPrompt(stats))
      toast.push('success', t('settings.trace.analyzeSucceeded', { title: created.title }))
    } catch (reason) {
      toast.push('error', t('settings.trace.analyzeFailed'), describeError(reason, translateNow))
    } finally {
      if (mounted.current) setStarting(false)
    }
  }

  const pickerLoading = threadsStatus === 'idle' || threadsStatus === 'loading'
  const statsLoaded = statsStatus === 'ready' && stats != null

  return (
    <SettingsSection
      title={t('settings.trace.title')}
      desc={t('settings.trace.desc')}
      actions={
        selectedId ? (
          <Button size="sm" disabled={statsStatus === 'loading'} onClick={reload}>
            {statsStatus === 'loading' ? t('common.refreshing') : t('settings.trace.reload')}
          </Button>
        ) : undefined
      }
    >
      {pickerLoading ? <Skeleton rows={1} /> : null}
      {threadsStatus === 'error' ? (
        <Notice tone="error" title={t('settings.trace.loadFailed')} />
      ) : null}
      {threadsStatus === 'ready' || threads.length > 0 ? (
        <ThreadPicker threads={threads} selectedId={selectedId} onSelect={setSelectedId} loading={pickerLoading} />
      ) : null}

      {!selectedId ? (
        <EmptyState title={t('settings.trace.emptyTitle')} hint={t('settings.trace.emptyHint')} />
      ) : null}

      {selectedId && statsStatus === 'loading' ? <Skeleton rows={4} /> : null}
      {selectedId && statsStatus === 'error' ? (
        <LoadError message={statsError ?? t('settings.trace.loadFailed')} onRetry={reload} />
      ) : null}
      {statsLoaded && stats ? (
        <>
          <MetricRow stats={stats} locale={locale} />
          <TraceCharts stats={stats} />
        </>
      ) : null}

      {/* The single trigger for session creation. Disabled until the stats of
          the selected thread are on screen — the prompt is built from them. */}
      <div className="ark-trace__actions">
        <Button
          variant="primary"
          disabled={!statsLoaded || starting}
          onClick={() => void startAnalysis()}
        >
          {starting ? t('settings.trace.analyzing') : t('settings.trace.analyze')}
        </Button>
        <span className="ark-settings__hint">{t('settings.trace.analyzeHint')}</span>
      </div>
    </SettingsSection>
  )
}
