/**
 * Per-turn clocks: the renderer's fold and the rule that keeps them honest.
 *
 * The turn header prints "执行过程 · N 步 · 耗时 X". That span used to be
 * re-derived from `GET /v1/debug/llm-rounds`, which is a **bounded
 * diagnostics page** (`LLM_ROUNDS_MAX_LIMIT`). In a long session the newest
 * turns fell past the page, they had no rows to sum, and the header rendered a
 * handful of seconds for a turn that really took minutes — the "122 steps in
 * 11 seconds" report. These tests pin the two properties that prevent it:
 * the seed covers every turn the server knows about, and a live turn
 * accumulates into its own entry instead of appending a second row.
 *
 * Since the `active_ms` field the seeded span is the server-folded wall clock
 * with user-gate waits excluded — the readout the header actually shows —
 * while `durationMs` stays the sum of measured model-call time.
 */
import { describe, expect, it } from 'vitest'
import type { ThreadUsageResponse } from '../../../shared/contracts'
import { appendRound, roundsFromTurnUsage, totalActiveMs, turnActiveMs, turnModelMs } from './rounds'

const T1 = '00000000-0000-0000-0000-0000000000a1'
const T2 = '00000000-0000-0000-0000-0000000000a2'
const T3 = '00000000-0000-0000-0000-0000000000a3'

function response(rows: {
  turn_id: string
  duration_ms?: number
  active_ms?: number
  samples?: number
}[]): ThreadUsageResponse {
  return {
    thread_id: '00000000-0000-0000-0000-0000000000ff',
    turns: rows.length,
    samples: rows.reduce((total, row) => total + (row.samples ?? 1), 0),
    turn_usage: rows.map((row) => ({
      turn_id: row.turn_id,
      samples: row.samples ?? 1,
      prompt_tokens: 100,
      completion_tokens: 10,
      total_tokens: 110,
      ...(row.duration_ms === undefined ? {} : { duration_ms: row.duration_ms }),
      ...(row.active_ms === undefined ? {} : { active_ms: row.active_ms })
    }))
  } as ThreadUsageResponse
}

describe('roundsFromTurnUsage', () => {
  it('keeps every turn the server folded, oldest first', () => {
    const rounds = roundsFromTurnUsage(
      response([
        { turn_id: T1, duration_ms: 1_004_000, active_ms: 1_100_000 },
        { turn_id: T2, duration_ms: 45_000, active_ms: 60_000 },
        { turn_id: T3, duration_ms: 3_000, active_ms: 5_000 }
      ])
    )
    expect(rounds).toEqual([
      { turnId: T1, durationMs: 1_004_000, activeMs: 1_100_000 },
      { turnId: T2, durationMs: 45_000, activeMs: 60_000 },
      { turnId: T3, durationMs: 3_000, activeMs: 5_000 }
    ])
    // The session total is the sum of the same rows.
    expect(totalActiveMs(rounds)).toBe(1_165_000)
  })

  it('does not depend on how many rounds came before a turn', () => {
    // The old page was capped at 500 rounds. 600 turns of 2s each must still
    // report 600 clocks; a turn's span is never "the remainder that fit".
    const rows = Array.from({ length: 600 }, (_, index) => ({
      turn_id: `00000000-0000-0000-0000-${String(index).padStart(12, '0')}`,
      duration_ms: 2_000,
      active_ms: 2_500
    }))
    const rounds = roundsFromTurnUsage(response(rows))
    expect(rounds).toHaveLength(600)
    expect(totalActiveMs(rounds)).toBe(1_500_000)
    // The newest turn keeps its full span — the reported symptom was exactly
    // that the newest turns lost theirs.
    expect(rounds.at(-1)?.activeMs).toBe(2_500)
  })

  it('keeps a usage-less turn for its active span', () => {
    // A turn that failed before the first model round has no duration but
    // real active time; its row must exist so the clock covers it.
    const rounds = roundsFromTurnUsage(
      response([
        { turn_id: T1, samples: 0, active_ms: 1_200 },
        { turn_id: T2, duration_ms: 5_000, active_ms: 6_000 }
      ])
    )
    expect(rounds).toEqual([
      { turnId: T1, durationMs: 0, activeMs: 1_200 },
      { turnId: T2, durationMs: 5_000, activeMs: 6_000 }
    ])
    expect(turnActiveMs(rounds, T1)).toBe(1_200)
  })

  it('treats an unmeasured turn as unknown instead of printing a confident 0', () => {
    // A log written before the fields existed has neither span.
    const rounds = roundsFromTurnUsage(response([{ turn_id: T1 }, { turn_id: T2, duration_ms: 5_000 }]))
    expect(turnActiveMs(rounds, T1)).toBeNull()
    expect(turnModelMs(rounds, T2)).toBe(5_000)
  })

  it('ignores non-finite and non-positive spans', () => {
    const rounds = roundsFromTurnUsage(
      response([
        { turn_id: T1, duration_ms: Number.NaN, active_ms: Number.NaN },
        { turn_id: T2, duration_ms: 0, active_ms: 0 }
      ])
    )
    expect(turnModelMs(rounds, T1)).toBe(0)
    expect(turnActiveMs(rounds, T1)).toBeNull()
    expect(turnActiveMs(rounds, T2)).toBe(0)
  })
})

describe('appendRound', () => {
  it('accumulates a live round into its own turn entry', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000, activeMs: 12_000 }]
    const live = appendRound(seeded, { turnId: T1, durationMs: 1_500 })
    // One entry per turn: a second row would double count the turn.
    expect(live).toHaveLength(1)
    expect(turnModelMs(live, T1)).toBe(11_500)
    // The active span is server-owned; live rounds never touch it.
    expect(turnActiveMs(live, T1)).toBe(12_000)
  })

  it('inserts a turn it has not seen yet at the end', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000, activeMs: null }]
    const live = appendRound(seeded, { turnId: T2, durationMs: 800 })
    expect(live.map((round) => round.turnId)).toEqual([T1, T2])
    expect(turnModelMs(live, T2)).toBe(800)
    // A live-first turn has no server-reported active span yet.
    expect(turnActiveMs(live, T2)).toBeNull()
  })

  it('does not mutate the caller’s list', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000, activeMs: null }]
    appendRound(seeded, { turnId: T1, durationMs: 5 })
    expect(seeded).toEqual([{ turnId: T1, durationMs: 10_000, activeMs: null }])
  })

  it('ignores a sample with no measurable span', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000, activeMs: null }]
    expect(appendRound(seeded, { turnId: T1, durationMs: 0 })).toBe(seeded)
  })
})
