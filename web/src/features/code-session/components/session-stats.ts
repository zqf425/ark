/**
 * Pure formatting for the session statistics strip.
 *
 * Wall time is owned by the runtime (`GET /v1/debug/llm-rounds`), not guessed
 * from renderer timestamps; this module only folds the reported samples and
 * renders a numeric clock so no user-facing prose is hardcoded here.
 */
import type { RoundTiming } from '../state/types'

/**
 * Localized unit words for `formatDurationWords`. Each one takes the count and
 * returns the rendered unit (`3时` / `3h`), so pluralization stays in the
 * catalog rather than in this module.
 */
export type DurationUnitWords = {
  hours: (count: number) => string
  minutes: (count: number) => string
  seconds: (count: number) => string
}

/**
 * Elapsed time spelled out with localized unit words (`10时45分3秒`).
 *
 * Units come from the caller's translator (the `formatRelative` pattern),
 * so this module stays free of React and of hardcoded user prose. Leading
 * zero units are dropped — `1时2秒` reads better than `1时0分2秒` — and a
 * sub-second span still prints `0秒` instead of an empty string.
 *
 * Deliberately NOT named `formatElapsed`: `state/live-activity.ts` already
 * exports that name for the in-flight `m:ss` readout, and two identical names
 * rendering different things is exactly the drift this repo forbids.
 */
export function formatDurationWords(
  ms: number,
  units: DurationUnitWords
): string {
  // `Math.max(0, NaN)` is still NaN, so non-finite spans are clamped first
  // rather than leaking `NaN秒` into the header.
  const total = Number.isFinite(ms) ? Math.floor(Math.max(0, ms) / 1000) : 0
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  const parts: string[] = []
  if (hours > 0) parts.push(units.hours(hours))
  // Minutes stay visible once hours are shown, so `1时0分` never collapses
  // into a misleading `1时`.
  if (hours > 0 || minutes > 0) parts.push(units.minutes(minutes))
  parts.push(units.seconds(seconds))
  return parts.join('')
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
