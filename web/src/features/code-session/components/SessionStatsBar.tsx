/**
 * Session statistics strip under the thread header.
 *
 * Single home for the canvas meta strip: authoritative session counters and
 * the live segments that used to sit above the composer (tok/s,
 * compaction notice). Everything numeric comes from the store's fold of the
 * authoritative `usage` events, seeded once from
 * `GET /v1/threads/{id}/usage` and extended live per sample, so the strip
 * updates without waiting for an HTTP round trip and the totals never describe
 * a narrower window than the session itself. The duration clock is the
 * aggregate's `active_ms` — wall clock with user-gate waits excluded (tool
 * execution, retries, recovery backoff and child runs all count; the user's
 * own thinking time does not) — extended live while a turn runs.
 * `GET /v1/debug/llm-rounds` is deliberately not read here: it is a bounded
 * diagnostics page, and summing it understated long sessions.
 */
import { useEffect, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { formatTokens, formatUsd } from '../../../shared/format'
import { getThreadUsage } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import { liveActiveMs } from './session-stats'
import { useClockTick } from './use-clock-tick'
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
  // the whole strip. Live samples update the aggregate in place; they are not
  // a signal to replay the usage log over HTTP.
  const usage = useCodeSessionStore((s) => s.usage)
  const usageTurns = useCodeSessionStore((s) => s.usageTurns)
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastCompaction = useCodeSessionStore((s) => s.lastCompaction)
  const [stats, setStats] = useState<Stats>(EMPTY_STATS)
  // The seed also back-fills the active-span channel (see `seedUsage`): a live
  // `usage` event that won the race leaves `activeMs` null, and without this
  // refresh the strip would never show a clock for the thread.
  const beginThreadUsage = useCodeSessionStore((s) => s.beginThreadUsage)
  const noteThreadUsage = useCodeSessionStore((s) => s.noteThreadUsage)
  const cancelThreadUsage = useCodeSessionStore((s) => s.cancelThreadUsage)
  const noteThreadRounds = useCodeSessionStore((s) => s.noteThreadRounds)

  useEffect(() => {
    if (!threadId) {
      setStats(EMPTY_STATS)
      return
    }
    const current = useCodeSessionStore.getState()
    // A detail load may already have seeded the aggregate. Live samples extend
    // it in the store, so mounting the strip must not replay the whole log.
    if (current.usage !== null && current.usageTurns !== null) return
    const request = beginThreadUsage(threadId)
    const controller = new AbortController()
    setStats((current) => (current.owner === threadId ? current : { owner: threadId, turns: null, roundsCount: null }))
    // One read, one truth: turns and model calls both come from the same
    // event-folded aggregate, so the strip can never report a call count that
    // disagrees with the turn count it is derived from.
    void getThreadUsage(threadId, controller.signal).then((usage) => {
      if (controller.signal.aborted || !noteThreadUsage(usage, request)) return
      setStats({ owner: threadId, turns: usage.turns, roundsCount: usage.samples })
      noteThreadRounds(usage)
    }).catch(() => {
      cancelThreadUsage(request)
      // Keep the last verified numbers: a diagnostics outage is not zero usage.
    })
    return () => {
      controller.abort()
      cancelThreadUsage(request)
    }
  }, [threadId, beginThreadUsage, cancelThreadUsage, noteThreadUsage, noteThreadRounds])

  if (!threadId) return null
  const visible = stats.owner === threadId ? stats : null
  const turns = visible?.turns ?? usageTurns ?? null
  // The store aggregate is authoritative and already includes every live sample;
  // an older HTTP count must not pin the display after SSE usage arrives.
  const roundsCount = usage?.samples ?? visible?.roundsCount ?? null
  // Total active span: the server-folded wall clock with user-gate waits
  // excluded, covering every turn in the log (including ones this client never
  // rendered). While the thread has a turn in flight the clock extends live
  // from the active turn's anchor; a pending gate freezes it, because the
  // user is thinking and the clock does not run against them.
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  // The gate's `requested_at` is the freeze instant: the clock may run up to
  // it, never past it. Earliest pending gate wins (one gate at a time by
  // construction, but the earliest is the safe bound).
  const gatePendingSince = useCodeSessionStore((s) => {
    if (activeTurnId == null) return null
    const stamps = [
      ...Object.values(s.approvals).filter((a) => a.turn_id === activeTurnId && a.status === 'pending').map((a) => Date.parse(a.requested_at)),
      ...Object.values(s.userInputs).filter((u) => u.turn_id === activeTurnId && u.status === 'pending').map((u) => Date.parse(u.requested_at))
    ].filter((ms) => Number.isFinite(ms))
    return stamps.length > 0 ? Math.min(...stamps) : null
  })
  const turnLastEventAt = useCodeSessionStore((s) => (activeTurnId ? s.turnLastEventAt[activeTurnId] ?? null : null))
  const nowMs = useClockTick(activeTurnId != null)
  // Total active span, derived from the store's per-turn rows (the seed's
  // `turn_usage` rows merged with live-only rows). The thread aggregate's
  // `active_ms` is only a fallback for older stores without per-turn rows:
  // summing rows handles the running turn directly — a seeded turn extends
  // from its server span, a live-only turn contributes its live estimate —
  // which is exactly the "890+ calls showed 2 minutes" fix: the old code
  // replaced the total with the live estimate alone and dropped the history.
  const allRounds = useCodeSessionStore((s) => s.rounds)
  const hasPerTurnRows = allRounds.length > 0
  const liveMs =
    activeTurnId != null ? liveActiveMs(allRounds, activeTurnId, nowMs, turnLastEventAt, gatePendingSince) : null
  const seedMs = usage?.activeMs ?? null
  const totalMs = (() => {
    if (hasPerTurnRows) {
      // Sum every turn: settled turns use their server span; the running turn
      // uses its live estimate (server span + extension, frozen at a gate).
      let settled = 0
      let running = 0
      for (const row of allRounds) {
        if (row.turnId === activeTurnId) running = liveMs ?? 0
        else settled += Math.max(0, row.activeMs ?? 0)
      }
      return settled + running
    }
    // Older fold without per-turn rows: the thread aggregate is all we have,
    // extended live if a turn is in flight (its span is inside the seed).
    if (liveMs != null) return Math.max(seedMs ?? 0, liveMs)
    return seedMs ?? 0
  })()
  const totalClock = useDurationFormat(totalMs, roundsCount != null && totalMs > 0)

  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)

  // Cache hit rate mirrors the Rust aggregate: `hit / (hit + miss)` from the
  // pair, never faked from `hit / prompt_tokens`. An aggregate whose misses
  // were derived past a split-modality remainder (image/audio inputs) still
  // renders — marked approximate with "≈" — because a labeled estimate beats
  // hiding the fact that prompt cache is working.
  const cacheRateApproximate = usage?.cacheHitRateApproximate ?? false
  const cacheRate =
    usage?.cacheHitTokens != null && usage.cacheMissTokens != null && usage.cacheHitTokens + usage.cacheMissTokens > 0
      ? usage.cacheHitTokens / (usage.cacheHitTokens + usage.cacheMissTokens)
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
            <span className="ark-stats-bar__item" title={cacheRateApproximate ? t('canvas.usage.cacheApproxTitle') : t('canvas.usage.cacheTitle')}>
              {cacheRateApproximate
                ? t('canvas.usage.cacheApprox', { percent: number(Math.round(cacheRate * 100)) })
                : t('canvas.usage.cache', { percent: number(Math.round(cacheRate * 100)) })}
            </span>
          ) : null}
          {usage.costUsd != null ? (
            <span className="ark-stats-bar__item" title={t('canvas.stats.costTitle')}>
              {t('canvas.stats.cost', { cost: formatUsd(usage.costUsd, locale) })}
            </span>
          ) : null}
        </>
      ) : null}

      {/* Stream health (formerly the strip above the composer): tok/s,
          compaction and errors only — connection state and the seq watermark
          are plumbing, not session stats, so they stay hidden. */}
      {usage?.lastRoundTokensPerSec != null ? (
        <span className="ark-stats-bar__item is-mono" title={t('canvas.usage.speedTitle')}>
          {t('canvas.usage.speed', { rate: number(Math.round(usage.lastRoundTokensPerSec)) })}
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
      {streamError ? <span className="ark-stats-bar__item is-error">{streamError}</span> : null}
    </div>
  )
}
