/**
 * Localized unit words for elapsed-time readouts.
 *
 * Three surfaces render a duration (`TurnBlock`'s process header, the reply
 * card's title row, the session strip's total). Giving each of them its own
 * hour/minute/second lambdas duplicated the same three i18n keys three times
 * and let the wording drift apart; this hook is that mapping's only home.
 *
 * The units themselves stay in the catalog (`canvas.duration.*`) — no prose is
 * hardcoded here either.
 */
import { useMemo } from 'react'
import { useI18n } from '../../../shared/i18n'
import { formatDurationWords, type DurationUnitWords } from '../components/session-stats'

export type DurationFormat = {
  /** The label plus the spelled-out span (`耗时 1时2分3秒`). */
  labelled: string
  /** The span alone (`1时2分3秒`), for rows that already carry a label. */
  span: string
}

/**
 * @param ms      measured span in milliseconds
 * @param enabled when false, both members are empty strings — callers render
 *                nothing rather than a `0秒` placeholder
 */
export function useDurationFormat(ms: number, enabled = true): DurationFormat {
  const { t } = useI18n()
  return useMemo(() => {
    if (!enabled) return { labelled: '', span: '' }
    const units: DurationUnitWords = {
      hours: (count) => t('canvas.duration.hours', { count }),
      minutes: (count) => t('canvas.duration.minutes', { count }),
      seconds: (count) => t('canvas.duration.seconds', { count })
    }
    const span = formatDurationWords(ms, units)
    return { labelled: t('canvas.process.duration', { duration: span }), span }
    // `t` is stable per locale; the memo keeps a token stream from rebuilding
    // the unit bag on every render of a long turn.
  }, [enabled, ms, t])
}
