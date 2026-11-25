import { describe, expect, it } from 'vitest'
import type { RuntimeEvent, ThreadUsageResponse, Usage } from '../../../shared/contracts'
import { applyEvent, emptyItemsState } from './items-reducer'
import { appendUsageTail, foldUsage, seedUsage, usageTailSamples } from './usage'
import type { UsageTailNode, UsageTailSample } from './types'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_ID = '00000000-0000-0000-0000-000000000002'
const ITEM_ID = '00000000-0000-0000-0000-000000000003'

function sample(overrides: Partial<Usage>): Usage {
  const prompt_tokens = overrides.prompt_tokens ?? 100
  const completion_tokens = overrides.completion_tokens ?? 10
  return {
    prompt_tokens,
    completion_tokens,
    total_tokens: prompt_tokens + completion_tokens,
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

const SNAPSHOT_REQUEST = { owner: THREAD_ID, seq: 0, generation: 1, baseSamples: 0 }

function pendingUsage(): ReturnType<typeof emptyItemsState> {
  return { ...emptyItemsState(), usageRequest: SNAPSHOT_REQUEST }
}

function retainedTail(...samples: UsageTailSample[]): UsageTailNode | null {
  return samples.reduce<UsageTailNode | null>((tail, sample) => appendUsageTail(tail, sample), null)
}

describe('foldUsage optional aggregates', () => {
  it('sums hit/miss counters but permanently poisons an incomplete cache rate', () => {
    const first = foldUsage(null, sample({ cache_hit_tokens: 80, cache_miss_tokens: 20 }), 40)
    expect(first.cacheHitTokens).toBe(80)
    expect(first.cacheMissTokens).toBe(20)
    expect(first.cacheCountersIncomplete).toBe(false)
    expect(first.lastRoundTokensPerSec).toBe(40)

    // Rust Usage::merge keeps the individual sums, but one missing counter
    // makes the aggregate hit rate unknowable for the rest of the thread.
    const second = foldUsage(first, sample({ prompt_tokens: 50, completion_tokens: 5, total_tokens: 55, cache_hit_tokens: 30 }), null)
    expect(second.cacheHitTokens).toBe(110)
    expect(second.cacheMissTokens).toBe(20)
    expect(second.cacheCountersIncomplete).toBe(true)
    expect(second.lastRoundTokensPerSec).toBeNull()

    const third = foldUsage(second, sample({ cache_hit_tokens: 10, cache_miss_tokens: 5 }), null)
    expect(third.cacheHitTokens).toBe(120)
    expect(third.cacheMissTokens).toBe(25)
    expect(third.cacheCountersIncomplete).toBe(true)

    const noCache = foldUsage(null, sample({}))
    expect(noCache.cacheHitTokens).toBeNull()
    expect(noCache.cacheMissTokens).toBeNull()
    expect(noCache.cacheCountersIncomplete).toBe(true)
  })

  it('marks the aggregate approximate once any sample derived its miss', () => {
    const real = foldUsage(null, sample({ cache_hit_tokens: 80, cache_miss_tokens: 20 }))
    expect(real.cacheHitRateApproximate).toBe(false)

    // One approximate sample (image input, miss = prompt - hit) taints the
    // whole thread total, mirroring Rust `Usage::merge` stickiness.
    const mixed = foldUsage(real, sample({ cache_hit_tokens: 30, cache_miss_tokens: 10, cache_hit_rate_approximate: true }))
    expect(mixed.cacheHitTokens).toBe(110)
    expect(mixed.cacheMissTokens).toBe(30)
    expect(mixed.cacheHitRateApproximate).toBe(true)

    // An HTTP seed carrying the flag propagates too.
    const seeded = foldUsage(null, sample({ cache_hit_tokens: 5, cache_miss_tokens: 5, cache_hit_rate_approximate: true }))
    expect(seeded.cacheHitRateApproximate).toBe(true)
  })

  it('keeps duration unknown once any folded sample has no measured duration', () => {
    const measured = foldUsage(null, sample({ duration_ms: 25 }))
    expect(measured.durationMs).toBe(25)
    expect(measured.durationIncomplete).toBe(false)

    const missing = foldUsage(measured, sample({}))
    expect(missing.durationMs).toBeNull()
    expect(missing.durationIncomplete).toBe(true)

    const later = foldUsage(missing, sample({ duration_ms: 10 }))
    expect(later.durationMs).toBeNull()
    expect(later.durationIncomplete).toBe(true)
  })
})

describe('foldUsage context reading', () => {
  it('keeps the last valid prompt reading across empty provider samples', () => {
    const empty = sample({ prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 })
    const first = foldUsage(null, empty)
    expect(first.lastPromptTokens).toBeNull()

    const valid = foldUsage(first, sample({ prompt_tokens: 139_259, completion_tokens: 44, total_tokens: 139_303 }))
    expect(valid.lastPromptTokens).toBe(139_259)

    const afterEmpty = foldUsage(valid, empty)
    expect(afterEmpty.samples).toBe(3)
    expect(afterEmpty.promptTokens).toBe(139_259)
    expect(afterEmpty.lastPromptTokens).toBe(139_259)

    const next = foldUsage(afterEmpty, sample({ prompt_tokens: 139_333, completion_tokens: 13_583, total_tokens: 152_916 }))
    expect(next.lastPromptTokens).toBe(139_333)
  })
})

describe('seedUsage cache counters', () => {
  it('seeds sums and the latest valid context reading from the lifetime response', () => {
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 2,
      turn_usage: [],
      prompt_tokens: 10,
      completion_tokens: 4,
      total_tokens: 14,
      cache_hit_tokens: 6,
      cache_miss_tokens: 4,
      last_valid_prompt_tokens: 10
    }
    const seeded = seedUsage(pendingUsage(), response, SNAPSHOT_REQUEST)
    expect(seeded.usage?.cacheHitTokens).toBe(6)
    expect(seeded.usage?.cacheMissTokens).toBe(4)
    expect(seeded.usage?.cacheCountersIncomplete).toBe(false)
    expect(seeded.usage?.durationIncomplete).toBe(true)
    expect(seeded.usage?.lastPromptTokens).toBe(10)
    expect(seeded.usage?.lastRoundTokensPerSec).toBeNull()
  })

  it('backfills the context reading from the authoritative lifetime aggregate', () => {
    const live = foldUsage(null, sample({ prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }))
    const request = { ...SNAPSHOT_REQUEST, generation: 2, baseSamples: 1 }
    const state = { ...emptyItemsState(), usage: live, usageRequest: request }
    const seeded = seedUsage(state, {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 2,
      turn_usage: [],
      prompt_tokens: 139_259,
      completion_tokens: 44,
      total_tokens: 139_303,
      last_valid_prompt_tokens: 139_259
    }, request)
    expect(seeded.usage?.samples).toBe(2)
    expect(seeded.usage?.promptTokens).toBe(139_259)
    expect(seeded.usage?.lastPromptTokens).toBe(139_259)

    const legacy = seedUsage(pendingUsage(), {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 139_259,
      completion_tokens: 44,
      total_tokens: 139_303
    }, SNAPSHOT_REQUEST)
    expect(legacy.usage?.lastPromptTokens).toBeNull()
  })

  it('marks a non-empty server aggregate incomplete when optional totals are absent', () => {
    const seeded = seedUsage(pendingUsage(), {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 10,
      completion_tokens: 4,
      total_tokens: 14
    }, SNAPSHOT_REQUEST)
    expect(seeded.usage?.cacheCountersIncomplete).toBe(true)
    expect(seeded.usage?.durationIncomplete).toBe(true)
    expect(seeded.usage?.durationMs).toBeNull()
  })
})

describe('seedUsage snapshot ownership', () => {
  const request = { owner: THREAD_ID, seq: 1, generation: 7, baseSamples: 0 }

  it('derives covered tail samples from the response when no coverage seq is available', () => {
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))
    live = applyEvent(live, usageEvent(3, '2026-10-28T00:00:02Z', sample({ prompt_tokens: 70 })))
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 50,
      completion_tokens: 10,
      total_tokens: 60
    }

    const seeded = seedUsage(live, response, request as never)
    expect(seeded.usage?.samples).toBe(2)
    expect(seeded.usage?.promptTokens).toBe(120)
    expect(seeded.usage?.lastPromptTokens).toBe(70)
    expect(seeded.usageTail).toBeNull()
    expect(seeded.usageRequest).toBeNull()
  })

  it('ignores a response whose thread or generation no longer owns the snapshot', () => {
    const live = {
      ...emptyItemsState(),
      usageRequest: { ...request, generation: 8 }
    }
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 50,
      completion_tokens: 10,
      total_tokens: 60
    }

    expect(seedUsage(live, response, request as never)).toBe(live)
  })

  it('rejects a response for a different thread than its request owner', () => {
    const response: ThreadUsageResponse = {
      thread_id: '00000000-0000-0000-0000-0000000000ff',
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 50,
      completion_tokens: 10,
      total_tokens: 60
    }

    const live = {
      ...emptyItemsState(),
      usageRequest: request,
      usageTail: retainedTail({ seq: 2, usage: sample({}), roundTokensPerSec: null })
    }
    const rejected = seedUsage(live, response, request)
    expect(rejected.usage).toBe(live.usage)
    expect(rejected.usageRequest).toBeNull()
    expect(rejected.usageTail).toBeNull()
  })

  it('rejects a request watermark older than the last incorporated seed', () => {
    const live = {
      ...emptyItemsState(),
      usageRequest: { ...request, seq: 4 },
      usageSeedSeq: 5
    }
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 50,
      completion_tokens: 10,
      total_tokens: 60
    }

    const rejected = seedUsage(live, response, { ...request, seq: 4 })
    expect(rejected.usage).toBe(live.usage)
    expect(rejected.usageSeedSeq).toBe(5)
    expect(rejected.usageRequest).toBeNull()
  })

  it('does not replace newer live usage with a response older than the request base', () => {
    const current = foldUsage(foldUsage(null, sample({})), sample({}))
    const live = {
      ...emptyItemsState(),
      usage: current,
      usageRequest: { ...request, baseSamples: 2 }
    }
    const response: ThreadUsageResponse = {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 1,
      turn_usage: [],
      prompt_tokens: 50,
      completion_tokens: 10,
      total_tokens: 60
    }

    expect(seedUsage(live, response, { ...request, baseSamples: 2 }).usage).toBe(current)
  })

  it('retains live samples only while a snapshot request is in flight', () => {
    let idle = emptyItemsState()
    idle = applyEvent(idle, usageEvent(1, '2026-10-28T00:00:01Z', sample({})))
    idle = applyEvent(idle, usageEvent(2, '2026-10-28T00:00:02Z', sample({})))
    expect(idle.usageTail).toBeNull()

    let pending: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    pending = applyEvent(pending, usageEvent(2, '2026-10-28T00:00:01Z', sample({})))
    pending = applyEvent(pending, usageEvent(3, '2026-10-28T00:00:02Z', sample({})))
    expect(usageTailSamples(pending.usageTail).map((entry) => entry.seq)).toEqual([2, 3])
  })

  it('accepts an unseen lower-seq usage sample while its snapshot is pending', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 9, baseSamples: 0 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(3, '2026-10-28T00:00:02Z', sample({ prompt_tokens: 70 })))
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))
    const duplicate = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))

    expect(duplicate).toBe(live)
    expect(live.usage?.samples).toBe(2)
    const seeded = seedUsage(live, {
      thread_id: THREAD_ID,
      turns: 1,
      samples: 0,
      turn_usage: [],
      prompt_tokens: 0,
      completion_tokens: 0,
      total_tokens: 0
    }, request)
    expect(seeded.usage?.samples).toBe(2)
    expect(seeded.usage?.promptTokens).toBe(120)
    expect(seeded.usage?.lastPromptTokens).toBe(70)
  })

  it('appends a pending usage sample without enumerating the retained tail', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 10, baseSamples: 0 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({})))
    const retained = live.usageTail
    if (!retained) throw new Error('expected retained usage tail')
    const guarded = new Proxy(retained, {
      get(target, property, receiver) {
        if (property === Symbol.iterator || property === 'next') throw new Error('retained tail copy')
        return Reflect.get(target, property, receiver)
      }
    })

    const next = applyEvent({ ...live, usageTail: guarded }, usageEvent(3, '2026-10-28T00:00:02Z', sample({})))

    expect(next.usage?.samples).toBe(2)
    expect(usageTailSamples(retained)).toHaveLength(1)
  })
})

describe('seedUsage SSE race', () => {
  function response(): ThreadUsageResponse {
    return {
      thread_id: THREAD_ID,
      turns: 3,
      samples: 9,
      turn_usage: [],
      prompt_tokens: 9000,
      completion_tokens: 1000,
      total_tokens: 10000,
      cost_usd: 0.5,
      cache_hit_tokens: 600,
      cache_miss_tokens: 400,
      duration_ms: 1200,
      active_ms: 442_467
    }
  }

  it('retains server history and replays a live sample beyond response coverage', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 2, baseSamples: 8 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))
    const seeded = seedUsage(live, { ...response(), samples: 8 }, request)

    expect(seeded.usage?.samples).toBe(9)
    expect(seeded.usage?.promptTokens).toBe(9050)
    expect(seeded.usage?.completionTokens).toBe(1010)
    expect(seeded.usage?.totalTokens).toBe(10060)
    expect(seeded.usage?.activeMs).toBe(442_467)
    expect(seeded.usage?.lastPromptTokens).toBe(50)
  })

  it('uses the response log watermark when historical samples precede the request', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 5, baseSamples: 0 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))
    const responseWithWatermark = { ...response(), samples: 9, last_seq: 1 }

    const seeded = seedUsage(live, responseWithWatermark, request)

    expect(seeded.usage?.samples).toBe(10)
    expect(seeded.usage?.promptTokens).toBe(9050)
    expect(seeded.usage?.lastPromptTokens).toBe(50)
  })

  it('does not double count a live sample already included by the server response', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 3, baseSamples: 0 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(2, '2026-10-28T00:00:01Z', sample({ prompt_tokens: 50 })))
    const responseWithWatermark = { ...response(), last_seq: 2 }
    const seeded = seedUsage(live, responseWithWatermark, request)

    expect(seeded.usage?.samples).toBe(9)
    expect(seeded.usage?.promptTokens).toBe(9000)
    expect(seeded.usage?.completionTokens).toBe(1000)
    expect(seeded.usage?.totalTokens).toBe(10000)
    expect(seeded.usage?.lastPromptTokens).toBeNull()
  })

  it('replays an out-of-order retained tail in event sequence order', () => {
    const request = { owner: THREAD_ID, seq: 1, generation: 4, baseSamples: 0 }
    const live = {
      ...emptyItemsState(),
      usageRequest: request,
      usageTail: retainedTail(
        { seq: 3, usage: sample({ prompt_tokens: 70 }), roundTokensPerSec: 70 },
        { seq: 2, usage: sample({ prompt_tokens: 50 }), roundTokensPerSec: 50 }
      )
    }
    const seeded = seedUsage(live, { ...response(), samples: 1, prompt_tokens: 50 }, request)

    expect(seeded.usage?.samples).toBe(2)
    expect(seeded.usage?.promptTokens).toBe(120)
    expect(seeded.usage?.lastPromptTokens).toBe(70)
    expect(seeded.usage?.lastRoundTokensPerSec).toBe(70)
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
