import { describe, expect, it } from 'vitest'
import type { RuntimeEvent, ThreadUsageResponse, Usage } from '../../../shared/contracts'
import { applyEvent, emptyItemsState } from './items-reducer'
import { foldUsage, seedUsage } from './usage'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_ID = '00000000-0000-0000-0000-000000000002'
const ITEM_ID = '00000000-0000-0000-0000-000000000003'

function sample(overrides: Partial<Usage>): Usage {
  return {
    prompt_tokens: 100,
    completion_tokens: 10,
    total_tokens: 110,
    ...overrides
  }
}

function usageEvent(seq: number, timestamp: string, usage: Usage): RuntimeEvent {
  return { kind: 'usage', seq, thread_id: THREAD_ID, turn_id: TURN_ID, timestamp, usage }
}

function delta(seq: number, timestamp: string, text: string): RuntimeEvent {
  return {
    kind: 'assistant_text_delta',
    seq,
    thread_id: THREAD_ID,
    turn_id: TURN_ID,
    item_id: ITEM_ID,
    timestamp,
    delta: text
  }
}

describe('foldUsage cache counters', () => {
  it('sums hit/miss counters and stays null until a sample carries one', () => {
    const first = foldUsage(null, sample({ cache_hit_tokens: 80, cache_miss_tokens: 20 }), 40)
    expect(first.cacheHitTokens).toBe(80)
    expect(first.cacheMissTokens).toBe(20)
    expect(first.lastRoundTokensPerSec).toBe(40)

    // A sample without the miss counter contributes nothing but keeps the sum.
    const second = foldUsage(first, sample({ prompt_tokens: 50, completion_tokens: 5, total_tokens: 55, cache_hit_tokens: 30 }), null)
    expect(second.cacheHitTokens).toBe(110)
    expect(second.cacheMissTokens).toBe(20)
    expect(second.lastRoundTokensPerSec).toBeNull()

    const noCache = foldUsage(null, sample({}))
    expect(noCache.cacheHitTokens).toBeNull()
    expect(noCache.cacheMissTokens).toBeNull()
  })
})

describe('seedUsage cache counters', () => {
  it('seeds sums from the lifetime response and leaves the speed unset', () => {
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 2,
      turn_usage: [],
      prompt_tokens: 10,
      completion_tokens: 4,
      total_tokens: 14,
      cache_hit_tokens: 6,
      cache_miss_tokens: 4
    }
    const seeded = seedUsage(emptyItemsState(), response)
    expect(seeded.usage?.cacheHitTokens).toBe(6)
    expect(seeded.usage?.cacheMissTokens).toBe(4)
    expect(seeded.usage?.lastRoundTokensPerSec).toBeNull()
  })
})

describe('reducer round streaming window', () => {
  it('pairs provider completion tokens with the delta window to derive tok/s', () => {
    let state = emptyItemsState()
    state = applyEvent(state, delta(1, '2026-10-28T00:00:00.000Z', 'Hel'))
    state = applyEvent(state, delta(2, '2026-10-28T00:00:01.000Z', 'lo'))
    state = applyEvent(state, usageEvent(3, '2026-10-28T00:00:01.050Z', sample({ completion_tokens: 50 })))

    expect(state.usage?.lastRoundTokensPerSec).toBeCloseTo(50)
    expect(state.roundWindow).toBeNull()
  })

  it('shows no speed for a round without visible streaming', () => {
    const state = applyEvent(emptyItemsState(), usageEvent(1, '2026-10-28T00:00:00.000Z', sample({})))
    expect(state.usage?.lastRoundTokensPerSec).toBeNull()
  })

  it('restarts the window at round and turn boundaries', () => {
    let state = emptyItemsState()
    state = applyEvent(state, delta(1, '2026-10-28T00:00:00.000Z', 'round one'))
    state = applyEvent(state, usageEvent(2, '2026-10-28T00:00:00.500Z', sample({ completion_tokens: 10 })))

    state = applyEvent(state, delta(3, '2026-10-28T00:01:00.000Z', 'rou'))
    state = applyEvent(state, {
      kind: 'turn_completed',
      seq: 4,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:01:00.100Z'
    })
    expect(state.roundWindow).toBeNull()

    // The next turn measures its own round from scratch, not from the old window.
    state = applyEvent(state, {
      kind: 'turn_started',
      seq: 5,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:02:00.000Z',
      auto_continuation: false,
      auto_recovery: 0,
      attachment_ids: [],
      attachment_outcomes: []
    })
    state = applyEvent(state, delta(6, '2026-10-28T00:02:00.000Z', 'nd '))
    state = applyEvent(state, delta(7, '2026-10-28T00:02:01.000Z', 'two'))
    state = applyEvent(state, usageEvent(8, '2026-10-28T00:02:01.000Z', sample({ completion_tokens: 5 })))
    expect(state.usage?.lastRoundTokensPerSec).toBeCloseTo(5)
  })
})
