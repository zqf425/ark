import { describe, expect, it } from 'vitest'
import type { RoundTiming } from '../state/types'
import { formatDuration, turnDurationMs } from './session-stats'

function round(turnId: string, durationMs: number): RoundTiming {
  return { turnId, durationMs }
}

describe('session stats formatting', () => {
  it('sums one turn’s measured rounds and ignores other turns', () => {
    const rounds = [round('t1', 1200), round('t1', 800), round('t2', 5_000)]
    expect(turnDurationMs(rounds, 't1')).toBe(2_000)
    expect(turnDurationMs(rounds, 't2')).toBe(5_000)
    expect(turnDurationMs(rounds, 't3')).toBe(0)
  })

  it('ignores non-finite and negative samples instead of faking them', () => {
    const rounds = [round('t1', Number.NaN), round('t1', -10), round('t1', 250)]
    expect(turnDurationMs(rounds, 't1')).toBe(250)
  })

  it('renders a numeric clock below and above one hour', () => {
    expect(formatDuration(0)).toBe('0:00')
    expect(formatDuration(65_000)).toBe('1:05')
    expect(formatDuration(3_723_000)).toBe('1:02:03')
  })
})
