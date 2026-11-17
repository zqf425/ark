/**
 * Pure formatting for the session statistics strip.
 *
 * Wall time is owned by the runtime (`GET /v1/debug/llm-rounds`), not guessed
 * from renderer timestamps; this module only folds the reported samples and
 * renders a numeric clock so no user-facing prose is hardcoded here.
 */
import type { RoundTiming } from '../state/types'

/** Numeric clock only (`m:ss` / `h:mm:ss`); wording lives in the i18n catalog. */
export function formatDuration(ms: number): string {
  const seconds = Math.floor(Math.max(0, ms) / 1000)
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const rest = seconds % 60
  const pad = (value: number): string => String(value).padStart(2, '0')
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${minutes}:${pad(rest)}`
}

/**
 * Sum of one turn's measured model rounds. A live round without a recorded
 * duration contributes 0 rather than blocking the row's clock.
 */
export function turnDurationMs(rounds: readonly RoundTiming[], turnId: string): number {
  return rounds.reduce(
    (total, round) => (round.turnId === turnId && Number.isFinite(round.durationMs) ? total + Math.max(0, round.durationMs) : total),
    0
  )
}
