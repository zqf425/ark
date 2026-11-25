/**
 * Per-turn model and active spans. HTTP usage snapshots seed every turn; live
 * samples add model time while the reducer banks active wall time. Snapshot
 * merges are monotonic so a late response cannot roll either clock backwards.
 */
import type { ThreadUsageResponse, TurnId } from '../../../shared/contracts'
import type { RoundTiming } from './types'

export type { RoundTiming } from './types'

const laterSpan = (current: number | null, incoming: number | null): number | null =>
  current == null ? incoming : incoming == null ? current : Math.max(current, incoming)

/** Merge the complete server fold with newer live rows without regressing clocks. */
export function roundsFromTurnUsage(
  response: ThreadUsageResponse,
  previous: readonly RoundTiming[] = []
): RoundTiming[] {
  const prior = new Map(previous.map((row) => [row.turnId, row]))
  const seeded = (response.turn_usage ?? []).map((turn) => {
    const existing = prior.get(turn.turn_id)
    const durationMs =
      typeof turn.duration_ms === 'number' && Number.isFinite(turn.duration_ms) && turn.duration_ms > 0
        ? turn.duration_ms
        : 0
    const activeMs =
      typeof turn.active_ms === 'number' && Number.isFinite(turn.active_ms) ? turn.active_ms : null
    return {
      turnId: turn.turn_id,
      durationMs: Math.max(existing?.durationMs ?? 0, durationMs),
      activeMs: laterSpan(existing?.activeMs ?? null, activeMs)
    }
  })
  const seededIds = new Set(seeded.map((row) => row.turnId))
  return [...seeded, ...previous.filter((row) => !seededIds.has(row.turnId))]
}

/** Build the constant-time lookup kept alongside the ordered clock rows. */
export function indexRounds(rounds: readonly RoundTiming[]): Record<TurnId, number> {
  const index: Record<TurnId, number> = {}
  rounds.forEach((round, position) => {
    index[round.turnId] = position
  })
  return index
}

/** Add measured model time without changing the active span. */
export function appendRound(
  current: RoundTiming[],
  timing: { turnId: TurnId; durationMs: number },
  index?: Record<TurnId, number>
): RoundTiming[] {
  if (!(timing.durationMs > 0)) return current
  const position = index?.[timing.turnId]
  const existing = index === undefined
    ? current.find((round) => round.turnId === timing.turnId)
    : position === undefined ? undefined : current[position]
  if (!existing) {
    if (index) index[timing.turnId] = current.length
    return [...current, { turnId: timing.turnId, durationMs: timing.durationMs, activeMs: null }]
  }
  if (index === undefined || position === undefined) {
    return current.map((round) =>
      round.turnId === timing.turnId
        ? { ...round, durationMs: round.durationMs + timing.durationMs }
        : round
    )
  }
  const next = current.slice()
  next[position] = { ...existing, durationMs: existing.durationMs + timing.durationMs }
  return next
}

/** Sum of one turn's measured model rounds; rounds without a duration count as 0. */
export function turnModelMs(rounds: readonly RoundTiming[], turnId: TurnId): number {
  return rounds.reduce((total, round) => (round.turnId === turnId ? total + round.durationMs : total), 0)
}

/** One turn's banked or server-reported active span, or null when unknown. */
export function turnActiveMs(rounds: readonly RoundTiming[], turnId: TurnId): number | null {
  return rounds.find((round) => round.turnId === turnId)?.activeMs ?? null
}

/** Sum of every turn's active span; the strip's wall-clock-with-waits-excluded clock. */
export function totalActiveMs(rounds: readonly RoundTiming[]): number {
  return rounds.reduce((total, round) => total + Math.max(0, round.activeMs ?? 0), 0)
}
