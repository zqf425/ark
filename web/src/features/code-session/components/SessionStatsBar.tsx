/**
 * Session statistics strip under the thread header.
 *
 * Single home for the canvas meta strip: authoritative session counters and
 * the live segments that used to sit above the composer (tok/s, context fill,
 * compaction notice). Everything numeric comes from the store's fold of the
 * authoritative `usage` events, seeded once from
 * `GET /v1/threads/{id}/usage` and extended live per sample, so the strip
 * updates without waiting for an HTTP round trip and the totals never describe
 * a narrower window than the session itself. The duration clock is the
 * aggregate's own `duration_ms` — model time only, never tool execution.
 * `GET /v1/debug/llm-rounds` is deliberately not read here: it is a bounded
 * diagnostics page, and summing it understated long sessions.
 */
import { useEffect, useMemo, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { formatTokens, formatUsd } from '../../../shared/format'
import { getThreadUsage } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import { hasLiveItems, type ItemView } from '../state/selectors'
import { useContextLimit } from '../hooks/use-context-limit'
import { useDurationFormat } from '../hooks/use-duration-words'

type Stats = {
  owner: string | null
  turns: number | null
  roundsCount: number | null
}

const EMPTY_STATS: Stats = { owner: null, turns: null, roundsCount: null }

export function SessionStatsBar({ threadId }: { threadId: string | null }): JSX.Element | null {
  const { t, locale } = useI18n()
  // One channel per slice: a stream error or gate event no longer re-renders
  // the whole strip, and `samples` is the refresh signal for the HTTP reads.
  const usage = useCodeSessionStore((s) => s.usage)
  const samples = usage?.samples ?? 0
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastCompaction = useCodeSessionStore((s) => s.lastCompaction)
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const live = useMemo(() => hasLiveItems({ itemOrder, itemsById } satisfies ItemView), [itemOrder, itemsById])
  const reading = useContextLimit()
  const [stats, setStats] = useState<Stats>(EMPTY_STATS)

  useEffect(() => {
    if (!threadId) {
      setStats(EMPTY_STATS)
      return
    }
    const controller = new AbortController()
    setStats((current) => (current.owner === threadId ? current : { owner: threadId, turns: null, roundsCount: null }))
    // One read, one truth: turns and model calls both come from the same
    // event-folded aggregate, so the strip can never report a call count that
    // disagrees with the turn count it is derived from.
    void getThreadUsage(threadId, controller.signal).then((usage) => {
      if (controller.signal.aborted) return
      setStats({ owner: threadId, turns: usage.turns, roundsCount: usage.samples })
    }).catch(() => {
      // Keep the last verified numbers: a diagnostics outage is not zero usage.
    })
    return () => controller.abort()
  }, [threadId, samples])

  if (!threadId) return null
  const visible = stats.owner === threadId ? stats : null
  const turns = visible?.turns ?? null
  const roundsCount = visible?.roundsCount ?? null
  // Total measured model time, in the same spelled-out units as the per-turn
  // clock so the session total and the turn totals read alike. It is the
  // aggregate's own `duration_ms` — not a sum over the renderer's per-turn
  // rows, which only cover turns the client has seen and would understate a
  // long session. The two agree because the same fold produces both.
  const totalMs = usage?.durationMs ?? 0
  const totalClock = useDurationFormat(totalMs, roundsCount != null)

  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)

  // Cache hit rate only from the hit/miss pair, mirroring the Rust aggregate:
  // never faked from `hit / prompt_tokens`, which would overstate efficiency.
  const cacheRate =
    usage?.cacheHitTokens != null && usage.cacheMissTokens != null && usage.cacheHitTokens + usage.cacheMissTokens > 0
      ? usage.cacheHitTokens / (usage.cacheHitTokens + usage.cacheMissTokens)
      : null
  // Fill against the model's real window; the warning tone starts at the
  // runtime's compaction threshold, not at a hardcoded fraction.
  const contextRatio =
    usage && reading.window && reading.window > 0 && usage.lastPromptTokens != null
      ? usage.lastPromptTokens / reading.window
      : null
  const contextThreshold =
    reading.window && reading.window > 0 && reading.threshold != null
      ? reading.threshold / reading.window
      : null

  return (
    <div className="ark-stats-bar" role="group" aria-label={t('canvas.stats.aria')}>
      {roundsCount != null ? (
        <>
          <span className="ark-stats-bar__item" title={t('canvas.stats.turnsTitle')}>
            {t('canvas.stats.turns', { count: number(turns ?? 0) })}
          </span>
          <span className="ark-stats-bar__item" title={t('canvas.stats.roundsTitle')}>
            {t('canvas.stats.rounds', { count: number(roundsCount) })}
          </span>
          <span
            className="ark-stats-bar__item"
            title={t('canvas.stats.durationTitle')}
          >
            {t('canvas.stats.duration', { duration: totalClock.span })}
          </span>
        </>
      ) : (
        <span className="ark-stats-bar__item" title={t('canvas.stats.aria')}>{t('canvas.stats.pending')}</span>
      )}
      {usage ? (
        <>
          <span className="ark-stats-bar__item" title={t('canvas.usage.title')}>
            {t('canvas.stats.tokens', {
              total: formatTokens(usage.totalTokens),
              prompt: formatTokens(usage.promptTokens),
              completion: formatTokens(usage.completionTokens)
            })}
          </span>
          {cacheRate != null ? (
            <span className="ark-stats-bar__item" title={t('canvas.usage.cacheTitle')}>
              {t('canvas.usage.cache', { percent: number(Math.round(cacheRate * 100)) })}
            </span>
          ) : null}
          {usage.costUsd != null ? (
            <span className="ark-stats-bar__item" title={t('canvas.stats.costTitle')}>
              {t('canvas.stats.cost', { cost: formatUsd(usage.costUsd, locale) })}
            </span>
          ) : null}
        </>
      ) : null}

      {/* Stream health (formerly the strip above the composer): tok/s, context
          fill, compaction and errors only — connection state and the seq
          watermark are plumbing, not session stats, so they stay hidden. */}
      {usage?.lastRoundTokensPerSec != null ? (
        <span className="ark-stats-bar__item is-mono" title={t('canvas.usage.speedTitle')}>
          {t('canvas.usage.speed', { rate: number(Math.round(usage.lastRoundTokensPerSec)) })}
        </span>
      ) : null}
      {contextRatio != null ? (
        <span
          className={`ark-stats-bar__item is-mono${contextThreshold != null && contextRatio >= contextThreshold ? ' is-warning' : ''}`}
          title={t('canvas.usage.contextTitle')}
        >
          {t('canvas.usage.context', {
            used: formatTokens(usage?.lastPromptTokens ?? 0),
            limit: formatTokens(reading.window ?? 0),
            percent: number(Math.round(contextRatio * 100))
          })}
        </span>
      ) : null}
      {lastCompaction ? (
        <span className="ark-stats-bar__item" title={t('canvas.compaction.title')}>
          {t('canvas.compaction.notice', {
            removed: number(lastCompaction.removedItems),
            chars: number(lastCompaction.checkpointChars)
          })}
        </span>
      ) : null}
      {live ? <span className="ark-stats-bar__item is-live">{t('canvas.footer.generating')}</span> : null}
      {streamError ? <span className="ark-stats-bar__item is-error">{streamError}</span> : null}
    </div>
  )
}
