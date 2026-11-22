/**
 * Per-turn active-time spans for the session strip and the turn total.
 *
 * One authoritative `usage` event closes exactly one model round (ark-server
 * diagnostics); the wire does not carry a bare `round` item, so the renderer
 * keeps its own lightweight fold: the thread usage aggregate seeds the settled
 * per-turn spans, and a live `usage` event appends the round it closes,
 * measured from the turn-start/round boundary to the event timestamp.
 * Millis-clock live values are estimates; the next HTTP refresh is
 * authoritative and replaces the list wholesale, so the two never drift.
 *
 * `RoundTiming` now carries the turn's **active** span — wall clock minus the
 * windows the turn sat on an approval / user-input gate (server-folded in
 * `active_ms`), plus harness recovery/continuation gaps — not the sum of
 * measured model-call time, which stays in `durationMs` for the "model time"
 * readout.
 */
import type { ThreadUsageResponse, TurnId } from '../../../shared/contracts'

/** One settled turn's clock as the renderer needs it. */
export type RoundTiming = {
  turnId: TurnId
  /**
   * Sum of measured model-call wall time, milliseconds (the old readout).
   * Still folded live from `usage` events; the seed carries the server's sum.
   */
  durationMs: number
  /**
   * Active span: wall clock from `turn_started` to the terminal event minus
   * user-gate waits, milliseconds. Seeded from the server aggregate's
   * `active_ms`; live turns extend it against the renderer clock while no
   * gate is pending. `null` until the server has reported one for the turn.
   */
  activeMs: number | null
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
 * keeps `durationMs: null` rather than faking 0: a turn with no clock beats a
 * confident lie. Turns that never produced usage (failed before the first
 * model round) still appear, because the server now emits their active spans.
 */
export function roundsFromTurnUsage(response: ThreadUsageResponse): RoundTiming[] {
  // `turn_usage` is required on the wire but defensively optional here: an
  // older runtime answers without the breakdown, and a missing array must
  // degrade to "no per-turn clocks" rather than throwing mid-render.
  return (response.turn_usage ?? []).map((turn) => ({
    turnId: turn.turn_id,
    durationMs:
      typeof turn.duration_ms === 'number' && Number.isFinite(turn.duration_ms) && turn.duration_ms > 0
        ? turn.duration_ms
        : 0,
    activeMs: typeof turn.active_ms === 'number' && Number.isFinite(turn.active_ms) ? turn.active_ms : null
  }))
}

/**
 * Add the round a live `usage` event closes to its turn's entry.
 *
 * The list is one entry per **turn**, not per round (the seed above is
 * per-turn), so a running turn accumulates into the single entry it already
 * has instead of growing a second row that would double count it. A turn that
 * has no entry yet is inserted at the end, which is chronological because the
 * log is append-only. `activeMs` is left untouched here: it is owned by the
 * server fold and the live tick, never by per-round additions.
 */
export function appendRound(current: RoundTiming[], timing: { turnId: TurnId; durationMs: number }): RoundTiming[] {
  if (!(timing.durationMs > 0)) return current
  const existing = current.find((round) => round.turnId === timing.turnId)
  if (!existing) {
    return [...current, { turnId: timing.turnId, durationMs: timing.durationMs, activeMs: null }]
  }
  // Replace in place: the entry keeps its position so the list stays ordered
  // by turn, which is what the strip and the header both assume.
  return current.map((round) =>
    round.turnId === timing.turnId
      ? { ...round, durationMs: round.durationMs + timing.durationMs }
      : round
  )
}

/** Sum of one turn's measured model rounds; rounds without a duration count as 0. */
export function turnModelMs(rounds: readonly RoundTiming[], turnId: TurnId): number {
  return rounds.reduce((total, round) => (round.turnId === turnId ? total + round.durationMs : total), 0)
}

/** One turn's active span from the fold; `null` when the server has not reported one. */
export function turnActiveMs(rounds: readonly RoundTiming[], turnId: TurnId): number | null {
  return rounds.find((round) => round.turnId === turnId)?.activeMs ?? null
}

/** Sum of every turn's active span; the strip's wall-clock-with-waits-excluded clock. */
export function totalActiveMs(rounds: readonly RoundTiming[]): number {
  return rounds.reduce((total, round) => total + Math.max(0, round.activeMs ?? 0), 0)
}
