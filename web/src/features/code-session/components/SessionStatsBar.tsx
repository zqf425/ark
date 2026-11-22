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
  // the whole strip, and `samples` is the refresh signal for the HTTP reads.
  const usage = useCodeSessionStore((s) => s.usage)
  const samples = usage?.samples ?? 0
  const streamError = useCodeSessionStore((s) => s.streamError)
  const lastCompaction = useCodeSessionStore((s) => s.lastCompaction)
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
  // Total active span: the server-folded wall clock with user-gate waits
  // excluded, covering every turn in the log (including ones this client never
  // rendered). While the thread has a turn in flight the clock extends live
  // from the active turn's anchor; a pending gate freezes it, because the
  // user is thinking and the clock does not run against them.
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const allRounds = useCodeSessionStore((s) => s.rounds)
  const turnLastEventAt = useCodeSessionStore((s) => (activeTurnId ? s.turnLastEventAt[activeTurnId] ?? null : null))
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
  const nowMs = useClockTick(activeTurnId != null)
  const seedMs = usage?.activeMs ?? null
  const liveMs =
    activeTurnId != null ? liveActiveMs(allRounds, activeTurnId, nowMs, turnLastEventAt, gatePendingSince) : null
  const totalMs = liveMs ?? seedMs ?? 0
  const totalClock = useDurationFormat(totalMs, roundsCount != null && totalMs > 0)

  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)

  // Cache hit rate only from the hit/miss pair, mirroring the Rust aggregate:
  // never faked from `hit / prompt_tokens`, which would overstate efficiency.
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
