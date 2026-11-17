import { describe, expect, it } from 'vitest'
import type { RoundTiming } from '../state/types'
import { formatDurationWords, turnDurationMs } from './session-stats'

function round(turnId: string, durationMs: number): RoundTiming {
  return { turnId, durationMs }
}

/** Unit words the formatter composes; the wiring under test is the folding. */
const ZH_UNITS = {
  hours: (count: number) => `${count}时`,
  minutes: (count: number) => `${count}分`,
  seconds: (count: number) => `${count}秒`
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

  it('spells elapsed time out in hours, minutes and seconds', () => {
    // 10m 45s — the shape the process header shows.
    expect(formatDurationWords(645_000, ZH_UNITS)).toBe('10分45秒')
    expect(formatDurationWords(3_723_000, ZH_UNITS)).toBe('1时2分3秒')
  })

  it('drops leading zero units but keeps a sub-second span visible', () => {
    expect(formatDurationWords(45_000, ZH_UNITS)).toBe('45秒')
    // Under a second still prints a unit instead of an empty string.
    expect(formatDurationWords(400, ZH_UNITS)).toBe('0秒')
    // A negative or non-finite span never renders as a negative clock.
    expect(formatDurationWords(-5_000, ZH_UNITS)).toBe('0秒')
    expect(formatDurationWords(Number.NaN, ZH_UNITS)).toBe('0秒')
  })

  it('keeps the minutes slot once hours are shown', () => {
    // `1时0分3秒` — collapsing to `1时3秒` would read as a different span.
    expect(formatDurationWords(3_603_000, ZH_UNITS)).toBe('1时0分3秒')
  })
})
