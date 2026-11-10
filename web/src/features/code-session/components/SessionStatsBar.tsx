/**
 * Session statistics strip at the top of the canvas.
 *
 * Tokens/turns come from `GET /v1/threads/{id}/usage`; wall-clock time comes
 * from `GET /v1/debug/llm-rounds`, which the server folds from the same
 * authoritative `usage` events. The renderer only formats verified counters.
 * A live usage event changes the store's sample count, which is the refresh
 * signal; a failed diagnostics read keeps the last verified values instead of
 * silently replacing them with zeros.
 */
import { useEffect, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { formatTokens, formatUsd } from '../../../shared/format'
import type { LlmRound, ThreadUsageResponse } from '../../../shared/contracts'
import { getThreadUsage } from '../api/threads-api'
import { listLlmRounds } from '../api/llm-rounds-api'
import { useCodeSessionStore } from '../state/store'
import { countTurns, formatDuration, totalDurationMs } from './session-stats'

type Stats = {
  owner: string | null
  usage: ThreadUsageResponse | null
  rounds: LlmRound[] | null
  truncated: boolean
}

export function SessionStatsBar({ threadId }: { threadId: string | null }): JSX.Element | null {
  const { t, locale } = useI18n()
  const samples = useCodeSessionStore((s) => s.usage?.samples ?? 0)
  const [stats, setStats] = useState<Stats>({ owner: null, usage: null, rounds: null, truncated: false })

  useEffect(() => {
    if (!threadId) {
      setStats({ owner: null, usage: null, rounds: null, truncated: false })
      return
    }
    const controller = new AbortController()
    setStats((current) => current.owner === threadId
      ? current
      : { owner: threadId, usage: null, rounds: null, truncated: false })
    void Promise.all([
      getThreadUsage(threadId, controller.signal),
      listLlmRounds(threadId, controller.signal)
    ]).then(([usage, response]) => {
      if (controller.signal.aborted) return
      setStats({ owner: threadId, usage, rounds: response.rounds, truncated: response.truncated })
    }).catch(() => {
      // Keep the last verified numbers: a diagnostics outage is not zero usage.
    })
    return () => controller.abort()
  }, [threadId, samples])

  if (!threadId) return null
  const visible = stats.owner === threadId ? stats : null
  const usage = visible?.usage ?? null
  const rounds = visible?.rounds ?? null
  const truncated = visible?.truncated ?? false
  if (!usage || !rounds) {
    return <div className="ark-stats-bar ark-stats-bar--pending" role="status">{t('canvas.stats.pending')}</div>
  }

  const number = (value: number): string => new Intl.NumberFormat(locale).format(value)
  const duration = formatDuration(totalDurationMs(rounds))
  const cacheRate = usage.cache_hit_rate

  return (
    <div className="ark-stats-bar" role="group" aria-label={t('canvas.stats.aria')}>
      <span className="ark-stats-bar__item" title={t('canvas.stats.turnsTitle')}>
        {t('canvas.stats.turns', { count: number(Math.max(usage.turns, countTurns(rounds))) })}
      </span>
      <span
        className="ark-stats-bar__item"
        title={truncated ? t('canvas.stats.truncatedTitle') : t('canvas.stats.roundsTitle')}
      >
        {truncated
          ? t('canvas.stats.roundsTruncated', { count: number(rounds.length) })
          : t('canvas.stats.rounds', { count: number(rounds.length) })}
      </span>
      <span className="ark-stats-bar__item" title={t('canvas.stats.durationTitle')}>
        {t('canvas.stats.duration', { duration })}
      </span>
      <span className="ark-stats-bar__item" title={t('canvas.usage.title')}>
        {t('canvas.stats.tokens', {
          total: formatTokens(usage.total_tokens),
          prompt: formatTokens(usage.prompt_tokens),
          completion: formatTokens(usage.completion_tokens)
        })}
      </span>
      {cacheRate != null ? (
        <span className="ark-stats-bar__item" title={t('canvas.stats.cacheTitle')}>
          {t('canvas.stats.cacheRate', { percent: number(Math.round(cacheRate * 100)) })}
        </span>
      ) : null}
      {usage.cost_usd != null ? (
        <span className="ark-stats-bar__item" title={t('canvas.stats.costTitle')}>
          {t('canvas.stats.cost', { cost: formatUsd(usage.cost_usd, locale) })}
        </span>
      ) : null}
    </div>
  )
}
