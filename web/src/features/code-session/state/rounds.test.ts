/**
 * Per-turn model time: the renderer's fold and the rule that keeps it honest.
 *
 * The turn header prints "执行过程 · N 步 · 耗时 X". That span used to be
 * re-derived from `GET /v1/debug/llm-rounds`, which is a **bounded
 * diagnostics page** (`LLM_ROUNDS_MAX_LIMIT`). In a long session the newest
 * turns fell past the page, they had no rows to sum, and the header rendered a
 * handful of seconds for a turn that really took minutes — the "122 steps in
 * 11 seconds" report. These tests pin the two properties that prevent it:
 * the seed covers every turn the server knows about, and a live turn
 * accumulates into its own entry instead of appending a second row.
 */
import { describe, expect, it } from 'vitest'
import type { ThreadUsageResponse } from '../../../shared/contracts'
import { appendRound, roundsFromTurnUsage, totalRoundsMs, turnDurationMs } from './rounds'

const T1 = '00000000-0000-0000-0000-0000000000a1'
const T2 = '00000000-0000-0000-0000-0000000000a2'
const T3 = '00000000-0000-0000-0000-0000000000a3'

function response(rows: { turn_id: string; duration_ms?: number; samples?: number }[]): ThreadUsageResponse {
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
      ...(row.duration_ms === undefined ? {} : { duration_ms: row.duration_ms })
    }))
  } as ThreadUsageResponse
}

describe('roundsFromTurnUsage', () => {
  it('keeps every turn the server folded, oldest first', () => {
    const rounds = roundsFromTurnUsage(
      response([
        { turn_id: T1, duration_ms: 1_004_000 },
        { turn_id: T2, duration_ms: 45_000 },
        { turn_id: T3, duration_ms: 3_000 }
      ])
    )
    expect(rounds).toEqual([
      { turnId: T1, durationMs: 1_004_000 },
      { turnId: T2, durationMs: 45_000 },
      { turnId: T3, durationMs: 3_000 }
    ])
    // The session total is the sum of the same rows.
    expect(totalRoundsMs(rounds)).toBe(1_052_000)
  })

  it('does not depend on how many rounds came before a turn', () => {
    // The old page was capped at 500 rounds. 600 turns of 2s each must still
    // report 600 clocks; a turn's span is never "the remainder that fit".
    const rows = Array.from({ length: 600 }, (_, index) => ({
      turn_id: `00000000-0000-0000-0000-${String(index).padStart(12, '0')}`,
      duration_ms: 2_000
    }))
    const rounds = roundsFromTurnUsage(response(rows))
    expect(rounds).toHaveLength(600)
    expect(totalRoundsMs(rounds)).toBe(1_200_000)
    // The newest turn keeps its full span — the reported symptom was exactly
    // that the newest turns lost theirs.
    expect(rounds.at(-1)?.durationMs).toBe(2_000)
  })

  it('drops an unmeasured turn instead of printing a confident 0秒', () => {
    // A log written before the loop measured model time has no duration.
    const rounds = roundsFromTurnUsage(
      response([{ turn_id: T1, duration_ms: undefined }, { turn_id: T2, duration_ms: 5_000 }])
    )
    expect(rounds).toEqual([{ turnId: T2, durationMs: 5_000 }])
  })

  it('ignores non-finite and non-positive spans', () => {
    const rounds = roundsFromTurnUsage(
      response([{ turn_id: T1, duration_ms: Number.NaN }, { turn_id: T2, duration_ms: 0 }])
    )
    expect(rounds).toEqual([])
  })
})

describe('appendRound', () => {
  it('accumulates a live round into its own turn entry', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000 }]
    const live = appendRound(seeded, { turnId: T1, durationMs: 1_500 })
    // One entry per turn: a second row would double count the turn.
    expect(live).toHaveLength(1)
    expect(turnDurationMs(live, T1)).toBe(11_500)
  })

  it('inserts a turn it has not seen yet at the end', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000 }]
    const live = appendRound(seeded, { turnId: T2, durationMs: 800 })
    expect(live.map((round) => round.turnId)).toEqual([T1, T2])
    expect(turnDurationMs(live, T2)).toBe(800)
  })

  it('does not mutate the caller’s list', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000 }]
    appendRound(seeded, { turnId: T1, durationMs: 5 })
    expect(seeded).toEqual([{ turnId: T1, durationMs: 10_000 }])
  })

  it('ignores a sample with no measurable span', () => {
    const seeded = [{ turnId: T1, durationMs: 10_000 }]
    expect(appendRound(seeded, { turnId: T1, durationMs: 0 })).toBe(seeded)
  })
})