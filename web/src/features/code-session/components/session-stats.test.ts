import { describe, expect, it } from 'vitest'
import type { LlmRound } from '../../../shared/contracts'
import { countTurns, formatDuration, totalDurationMs } from './session-stats'

function round(turnId: string, durationMs: number): LlmRound {
  return {
    thread_id: '00000000-0000-0000-0000-000000000001',
    turn_id: turnId,
    round: 1,
    started_at: '2026-01-01T00:00:00Z',
    ended_at: '2026-01-01T00:00:01Z',
    duration_ms: durationMs,
    errors: [],
    prompt_tokens: 10,
    completion_tokens: 5,
    total_tokens: 15
  }
}

describe('session stats formatting', () => {
  it('sums reported durations and ignores invalid samples', () => {
    const rounds = [round('t1', 1200), round('t1', 800), round('t2', 5_000)]
    expect(totalDurationMs(rounds)).toBe(7_000)
    expect(totalDurationMs([round('t1', Number.NaN), round('t2', -10)])).toBe(0)
  })

  it('renders a numeric clock below and above one hour', () => {
    expect(formatDuration(0)).toBe('0:00')
    expect(formatDuration(65_000)).toBe('1:05')
    expect(formatDuration(3_723_000)).toBe('1:02:03')
  })

  it('counts distinct turns across rounds', () => {
    expect(countTurns([round('t1', 1), round('t1', 2), round('t2', 3)])).toBe(2)
  })
})
