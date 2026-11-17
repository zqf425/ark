/**
 * Per-round model-call durations for the session strip and the turn total.
 *
 * One authoritative `usage` event closes exactly one model round (ark-server
 * diagnostics); the wire does not carry a bare `round` item, so the renderer
 * keeps its own lightweight fold: the thread usage aggregate seeds the settled
 * per-turn spans, and a live `usage` event appends the round it closes,
 * measured from the turn-start/round boundary to the event timestamp.
 * Millis-clock live values are estimates; the next HTTP refresh is
 * authoritative and replaces the list wholesale, so the two never drift.
 */
import type { ThreadUsageResponse, TurnId } from '../../../shared/contracts'

/** One completed model round as the renderer needs it. */
export type RoundTiming = {
  turnId: TurnId
  /** Wall-clock duration of the model call itself, milliseconds. */
  durationMs: number
}

/**
 * Fold the thread's authoritative per-turn usage into the renderer's list.
 *
 * Deliberately *not* `GET /v1/debug/llm-rounds`: that route is a bounded
 * diagnostics page (`LLM_ROUNDS_MAX_LIMIT`), so in a long session whole turns
 * fell past the page and their clocks quietly rendered as a few seconds —
 * the "100+ steps in 11 seconds" report. The thread usage aggregate replays
 * the entire log per turn, so every turn keeps its real span.
 *
 * A turn with no measured duration (a log written before the field existed)
 * is dropped rather than faked as 0: a turn with no clock beats a confident
 * lie. Summing these rows reproduces the thread's own `duration_ms`.
 */
export function roundsFromTurnUsage(response: ThreadUsageResponse): RoundTiming[] {
  // `turn_usage` is required on the wire but defensively optional here: an
  // older runtime answers without the breakdown, and a missing array must
  // degrade to "no per-turn clocks" rather than throwing mid-render.
  return (response.turn_usage ?? []).flatMap((turn) => {
    const ms = turn.duration_ms
    return typeof ms === 'number' && Number.isFinite(ms) && ms > 0
      ? [{ turnId: turn.turn_id, durationMs: ms }]
      : []
  })
}

/**
 * Add the round a live `usage` event closes to its turn's entry.
 *
 * The list is one entry per **turn**, not per round (the seed above is
 * per-turn), so a running turn accumulates into the single entry it already
 * has instead of growing a second row that would double count it. A turn that
 * has no entry yet is inserted at the end, which is chronological because the
 * log is append-only.
 */
export function appendRound(current: RoundTiming[], timing: RoundTiming): RoundTiming[] {
  if (!(timing.durationMs > 0)) return current
  const existing = current.find((round) => round.turnId === timing.turnId)
  if (!existing) return [...current, timing]
  // Replace in place: the entry keeps its position so the list stays ordered
  // by turn, which is what the strip and the header both assume.
  return current.map((round) =>
    round.turnId === timing.turnId
      ? { turnId: round.turnId, durationMs: round.durationMs + timing.durationMs }
      : round
  )
}

/** Sum of one turn's round durations; rounds without a duration count as 0. */
export function turnDurationMs(rounds: readonly RoundTiming[], turnId: TurnId): number {
  return rounds.reduce((total, round) => (round.turnId === turnId ? total + round.durationMs : total), 0)
}

/** Sum over every round; the strip's "model time only" clock. */
export function totalRoundsMs(rounds: readonly RoundTiming[]): number {
  return rounds.reduce((total, round) => total + Math.max(0, round.durationMs), 0)
}
