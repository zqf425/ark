/**
 * Per-round model-call durations for the session strip and the turn total.
 *
 * One authoritative `usage` event closes exactly one model round (ark-server
 * diagnostics); the wire does not carry a bare `round` item, so the renderer
 * keeps its own lightweight fold: the HTTP snapshot seeds the settled rounds,
 * and a live `usage` event appends the round it closes, measured from the
 * turn-start/round boundary to the event timestamp. Millis-clock live values
 * are estimates; the next HTTP refresh is authoritative and replaces the list
 * wholesale, so the two never accumulate drift.
 */
import type { LlmRoundsResponse, TurnId } from '../../../shared/contracts'

/** One completed model round as the renderer needs it. */
export type RoundTiming = {
  turnId: TurnId
  /** Wall-clock duration of the model call itself, milliseconds. */
  durationMs: number
}

/**
 * Fold the diagnostics snapshot into the renderer's list. Rounds arrive
 * oldest-last per turn but sorted globally by start; bad samples are dropped
 * rather than faked as 0 (the strip would otherwise print a confident lie).
 */
export function roundsFromSnapshot(response: LlmRoundsResponse): RoundTiming[] {
  return response.rounds.flatMap((round) => {
    const ms = round.duration_ms
    // The projection always closes a round with a duration; a missing one
    // means a malformed sample, dropped rather than faked as 0.
    return typeof ms === 'number' && Number.isFinite(ms) && ms > 0
      ? [{ turnId: round.turn_id, durationMs: ms }]
      : []
  })
}

/**
 * Append or replace the round a live `usage` event closes. A turn streams one
 * usage event per round, so a running turn's list grows by one per event; a
 * refreshed snapshot for the same thread replaces the list wholesale.
 */
export function appendRound(current: RoundTiming[], timing: RoundTiming): RoundTiming[] {
  if (!(timing.durationMs > 0)) return current
  return [...current, timing]
}

/** Sum of one turn's round durations; rounds without a duration count as 0. */
export function turnDurationMs(rounds: readonly RoundTiming[], turnId: TurnId): number {
  return rounds.reduce((total, round) => (round.turnId === turnId ? total + round.durationMs : total), 0)
}

/** Sum over every round; the strip's "model time only" clock. */
export function totalRoundsMs(rounds: readonly RoundTiming[]): number {
  return rounds.reduce((total, round) => total + Math.max(0, round.durationMs), 0)
}
