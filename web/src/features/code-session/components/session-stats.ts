/**
 * Pure formatting for the session statistics strip.
 *
 * Wall time is owned by the runtime (`GET /v1/debug/llm-rounds`), not guessed
 * from renderer timestamps; this module only folds the reported samples and
 * renders a numeric clock so no user-facing prose is hardcoded here.
 */
import type { LlmRound } from '../../../shared/contracts'

/** Sum of completed model-call durations; bad samples are ignored, not faked. */
export function totalDurationMs(rounds: readonly LlmRound[]): number {
  return rounds.reduce((total, round) => {
    const value = round.duration_ms
    return Number.isFinite(value) ? total + Math.max(0, value) : total
  }, 0)
}

/** Numeric clock only (`m:ss` / `h:mm:ss`); wording lives in the i18n catalog. */
export function formatDuration(ms: number): string {
  const seconds = Math.floor(Math.max(0, ms) / 1000)
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const rest = seconds % 60
  const pad = (value: number): string => String(value).padStart(2, '0')
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${minutes}:${pad(rest)}`
}

/** Unique turns that produced at least one model round. */
export function countTurns(rounds: readonly LlmRound[]): number {
  return new Set(rounds.map((round) => round.turn_id)).size
}
