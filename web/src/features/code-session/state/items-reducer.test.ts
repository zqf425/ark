import { describe, expect, it, vi } from 'vitest'
import type { RuntimeEvent } from '../../../shared/contracts'
import { childCompletionStatusKey } from '../child-completion-status'
import { applyEvent, emptyItemsState, markApprovalLocally, seedFromItems } from './items-reducer'
import { seedUsage } from './usage'

function childCompleted(seq: number): RuntimeEvent {
  return {
    kind: 'child_completed',
    seq,
    thread_id: '00000000-0000-0000-0000-000000000001',
    timestamp: '2026-10-28T00:00:00Z',
    notification_id: `child:${seq}`,
    child_id: '00000000-0000-0000-0000-000000000002',
    label: 'inspect runtime',
    status: 'completed',
    summary: 'healthy'
  }
}

describe('items reducer child completions', () => {
  it('maps terminal child status to localized message keys', () => {
    expect(childCompletionStatusKey('queued')).toBe('canvas.child.status.queued')
    expect(childCompletionStatusKey('running')).toBe('canvas.child.status.running')
    expect(childCompletionStatusKey('completed')).toBe('canvas.child.status.completed')
    expect(childCompletionStatusKey('failed')).toBe('canvas.child.status.failed')
    expect(childCompletionStatusKey('aborted')).toBe('canvas.child.status.aborted')
    expect(childCompletionStatusKey('something-else')).toBe('canvas.child.status.ended')
  })

  it('only refreshes child supervision for child lifecycle events', () => {
    const start = emptyItemsState()
    const activity = applyEvent(start, {
      kind: 'child_activity',
      seq: 1,
      thread_id: '00000000-0000-0000-0000-000000000001',
      timestamp: '2026-10-28T00:00:00Z',
      child_id: '00000000-0000-0000-0000-000000000002',
      label: 'inspect runtime',
      status: 'running'
    })
    const text = applyEvent(activity, {
      kind: 'assistant_text_delta',
      seq: 2,
      thread_id: '00000000-0000-0000-0000-000000000001',
      turn_id: '00000000-0000-0000-0000-000000000003',
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:01Z',
      delta: 'still working'
    })
    const completed = applyEvent(text, childCompleted(3))

    expect(activity.childRunRevision).toBe(1)
    expect(text.childRunRevision).toBe(1)
    expect(completed.childRunRevision).toBe(2)
  })
})

describe('items reducer metadata events', () => {
  it('advances the watermark for a handled thread rename without warning about version skew', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    try {
      const state = applyEvent(emptyItemsState(), {
        kind: 'thread_updated',
        seq: 7,
        thread_id: '00000000-0000-0000-0000-000000000001',
        timestamp: '2026-10-28T00:00:00Z',
        title: 'renamed'
      })

      expect(state.lastSeq).toBe(7)
      expect(state.itemsById).toEqual({})
      expect(warn).not.toHaveBeenCalled()
    } finally {
      warn.mockRestore()
    }
  })
})

describe('items reducer runtime snapshot', () => {
  it('restores the active turn and only server-authorized pending gates', () => {
    const state = seedFromItems([], 8, {
      active_turn: {
        turn_id: '00000000-0000-0000-0000-000000000003',
        thread_id: '00000000-0000-0000-0000-000000000001',
        state: 'running',
        started_at: '2026-10-28T00:00:00Z'
      },
      pending_approvals: [{
        approval_id: 'approval-1',
        turn_id: '00000000-0000-0000-0000-000000000003',
        tool: 'bash',
        summary: 'run command',
        requested_at: '2026-10-28T00:00:01Z'
      }],
      pending_user_inputs: [{
        request_id: 'input-1',
        turn_id: '00000000-0000-0000-0000-000000000003',
        prompt: 'pick a branch',
        requested_at: '2026-10-28T00:00:02Z'
      }]
    })

    expect(state.activeTurnId).toBe('00000000-0000-0000-0000-000000000003')
    expect(state.turnState['00000000-0000-0000-0000-000000000003']).toBe('running')
    expect(state.approvals['approval-1']?.status).toBe('pending')
    expect(state.userInputs['input-1']?.status).toBe('pending')
  })
})

describe('items reducer thread_rewound', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'
  const ITEM_ID = '00000000-0000-0000-0000-000000000004'

  function seeded(): ReturnType<typeof seedFromItems> {
    return seedFromItems(
      [
        {
          item_id: ITEM_ID,
          turn_id: TURN_ID,
          kind: 'assistant_message',
          status: 'completed',
          created_at: '2026-10-28T00:00:00Z',
          text: 'discarded'
        }
      ],
      6,
      {
        pending_approvals: [{
          approval_id: 'approval-1',
          turn_id: TURN_ID,
          tool: 'bash',
          summary: 'run command',
          requested_at: '2026-10-28T00:00:01Z'
        }],
        pending_user_inputs: []
      }
    )
  }

  function rewound(seq: number, fromSeq: number): RuntimeEvent {
    return {
      kind: 'thread_rewound',
      seq,
      thread_id: THREAD_ID,
      timestamp: '2026-10-28T00:01:00Z',
      rewind_from_seq: fromSeq,
      target_turn_id: TURN_ID
    }
  }

  it('丢弃被截断后缀的投影、回合状态与 gate', () => {
    const state = applyEvent(seeded(), rewound(9, 3))

    expect(state.itemOrder).toEqual([])
    expect(state.itemsById[ITEM_ID]).toBeUndefined()
    expect(state.turnState[TURN_ID]).toBeUndefined()
    expect(state.approvals['approval-1']).toBeUndefined()
  })

  it('保留通知自身的不复用 seq 作为新水位，并标记待重建', () => {
    const state = applyEvent(seeded(), rewound(9, 3))

    // 序号不复用：通知 seq 已经高于被丢弃后缀曾用过的一切序号。
    expect(state.lastSeq).toBe(9)
    expect(state.rewindRevision).toBe(1)
    expect(state.rewoundFromSeq).toBe(3)
    // 子代理列表可能随后缀一起被丢弃，需要重拉。
    expect(state.childRunRevision).toBe(1)
  })

  it('旧 seq 的迟到事件不会把水位退回去', () => {
    const afterRewind = applyEvent(seeded(), rewound(9, 3))
    const late = applyEvent(afterRewind, {
      kind: 'assistant_text_delta',
      seq: 4,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      item_id: ITEM_ID,
      timestamp: '2026-10-28T00:00:02Z',
      delta: 'late'
    })

    expect(late.lastSeq).toBe(9)
    expect(late.rewindRevision).toBe(1)
  })

  it('重建后 seedFromItems 从 detail 水位恢复干净投影', () => {
    const afterRewind = applyEvent(seeded(), rewound(9, 3))
    const rebuilt = seedFromItems([], afterRewind.lastSeq, {
      pending_approvals: [],
      pending_user_inputs: []
    })

    expect(rebuilt.lastSeq).toBe(9)
    expect(rebuilt.itemOrder).toEqual([])
    expect(rebuilt.turnState).toEqual({})
  })
})

/**
 * Reference stability is the contract the canvas memoizes on: `StreamRow`,
 * `TurnBlock` and the per-channel selectors only bail out because a fold leaves
 * untouched channels and untouched items at the same object identity.
 */
describe('items reducer reference stability', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'
  const ITEM_A = '00000000-0000-0000-0000-00000000000a'
  const ITEM_B = '00000000-0000-0000-0000-00000000000b'

  function seeded(): ReturnType<typeof seedFromItems> {
    return seedFromItems(
      [
        { item_id: ITEM_A, turn_id: TURN_ID, kind: 'tool_call', status: 'completed', created_at: '2026-10-28T00:00:00Z' },
        { item_id: ITEM_B, turn_id: TURN_ID, kind: 'assistant_message', status: 'streaming', created_at: '2026-10-28T00:00:01Z', text: 'a' }
      ],
      2,
      { pending_approvals: [], pending_user_inputs: [] }
    )
  }

  function delta(seq: number, text: string): RuntimeEvent {
    return {
      kind: 'assistant_text_delta',
      seq,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      item_id: ITEM_B,
      timestamp: '2026-10-28T00:00:02Z',
      delta: text
    }
  }

  it('流式增量只替换 itemsById 与时间锚点，其余通道保持原引用', () => {
    const start = seeded()
    const startAnchors = { ...start.turnLastEventAt }
    const next = applyEvent(start, delta(3, 'b'))

    expect(next.itemsById).not.toBe(start.itemsById)
    expect(next.itemOrder).toBe(start.itemOrder)
    expect(next.turnState).toBe(start.turnState)
    expect(next.approvals).toBe(start.approvals)
    expect(next.userInputs).toBe(start.userInputs)
    expect(next.turnLastEventAt).not.toBe(start.turnLastEventAt)
    expect(start.turnLastEventAt).toEqual(startAnchors)
  })

  it('单个增量不枚举或复制整个 itemsById', () => {
    const start = seeded()
    const guarded = new Proxy(start.itemsById, {
      ownKeys: () => { throw new Error('full item map copy') }
    })
    const next = applyEvent({ ...start, itemsById: guarded }, delta(3, 'b'))

    expect(next.itemsById[ITEM_B]?.text).toBe('ab')
    expect(start.itemsById[ITEM_B]?.text).toBe('a')
  })

  it('单个增量不枚举历史回合的时间锚点', () => {
    const start = seeded()
    const guarded = new Proxy(start.turnLastEventAt, {
      ownKeys: () => { throw new Error('full turn anchor copy') }
    })
    const next = applyEvent({ ...start, turnLastEventAt: guarded }, delta(3, 'b'))

    expect(next.turnLastEventAt[TURN_ID]).toBe(Date.parse('2026-10-28T00:00:02Z'))
    expect(start.turnLastEventAt[TURN_ID]).toBeUndefined()
  })

  it('未变化的行对象在增量后保持同一引用', () => {
    const start = seeded()
    const next = applyEvent(start, delta(3, 'b'))

    expect(next.itemsById[ITEM_A]).toBe(start.itemsById[ITEM_A])
    expect(next.itemsById[ITEM_B]).not.toBe(start.itemsById[ITEM_B])
    expect(next.itemsById[ITEM_B]?.text).toBe('ab')
  })

  it('内容相同的 snapshot 重发不替换行对象，也不替换通道', () => {
    const start = seeded()
    const replayed = applyEvent(start, {
      kind: 'item_updated',
      seq: 3,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:02Z',
      item: {
        item_id: ITEM_B,
        turn_id: TURN_ID,
        kind: 'assistant_message',
        status: 'streaming',
        created_at: '2026-10-28T00:00:01Z',
        text: 'a'
      }
    })

    expect(replayed.itemsById).toBe(start.itemsById)
    expect(replayed.itemsById[ITEM_B]).toBe(start.itemsById[ITEM_B])
  })

  it('局部 item_updated 仍按字段合并，不会清空已累积文本', () => {
    const start = seeded()
    const completed = applyEvent(start, {
      kind: 'item_completed',
      seq: 3,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:02Z',
      item: { item_id: ITEM_B, turn_id: TURN_ID, kind: 'assistant_message', status: 'completed', created_at: '2026-10-28T00:00:01Z' }
    })

    expect(completed.itemsById[ITEM_B]?.text).toBe('a')
    expect(completed.itemsById[ITEM_B]?.status).toBe('completed')
    expect(completed.itemsById[ITEM_A]).toBe(start.itemsById[ITEM_A])
  })
})

describe('items reducer usage and compaction', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'

  function usageEvent(seq: number, promptTokens: number, completionTokens: number, costUsd?: number): RuntimeEvent {
    return {
      kind: 'usage',
      seq,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-11-02T00:00:00Z',
      usage: {
        prompt_tokens: promptTokens,
        completion_tokens: completionTokens,
        total_tokens: promptTokens + completionTokens,
        ...(costUsd != null ? { cost_usd: costUsd } : {})
      }
    }
  }

  it('按事件累加用量并记录最近一次 prompt 规模', () => {
    let state = emptyItemsState()
    state = applyEvent(state, usageEvent(1, 1000, 200))
    // No sample has carried a cost yet: the aggregate stays costless instead
    // of presenting 0 as a total.
    expect(state.usage?.costUsd).toBeNull()
    state = applyEvent(state, usageEvent(2, 1500, 300, 0.02))

    expect(state.usage).toEqual({
      samples: 2,
      promptTokens: 2500,
      completionTokens: 500,
      totalTokens: 3000,
      costUsd: 0.02,
      cacheHitTokens: null,
      cacheMissTokens: null,
      cacheCountersIncomplete: true,
      cacheHitRateApproximate: false,
      // Neither sample carried a measured duration: unknown, not `0秒`.
      durationMs: null,
      durationIncomplete: true,
      // The active span is server-owned; live folds keep the last seed value.
      activeMs: null,
      lastPromptTokens: 1500,
      // No deltas preceded these samples, so no streaming speed was measured.
      lastRoundTokensPerSec: null
    })
  })

  it('成本按 sum_opt_f 语义累加：带 cost 的首样本即生效，缺失样本保留累计值', () => {
    let state = emptyItemsState()
    state = applyEvent(state, usageEvent(1, 100, 10, 0.01))
    expect(state.usage?.costUsd).toBeCloseTo(0.01)
    state = applyEvent(state, usageEvent(2, 100, 10))
    expect(state.usage?.costUsd).toBeCloseTo(0.01)
    state = applyEvent(state, usageEvent(3, 100, 10, 0.01))
    expect(state.usage?.costUsd).toBeCloseTo(0.02)
  })

  it('记录最近一次压缩通知', () => {
    let state = emptyItemsState()
    state = applyEvent(state, {
      kind: 'compaction_completed',
      seq: 5,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-11-02T00:00:00Z',
      removed_items: 12,
      checkpoint_chars: 4800
    })

    expect(state.lastCompaction).toEqual({ removedItems: 12, checkpointChars: 4800, seq: 5 })
  })

  it('seedUsage 用服务端历史替换实时聚合，并按请求水位避免重复计数', () => {
    const request = { owner: THREAD_ID, seq: 0, generation: 1, baseSamples: 0 }
    let live: ReturnType<typeof emptyItemsState> = { ...emptyItemsState(), usageRequest: request }
    live = applyEvent(live, usageEvent(1, 1000, 200))
    const response = {
      thread_id: THREAD_ID,
      turns: 3,
      samples: 9,
      turn_usage: [],
      prompt_tokens: 9000,
      completion_tokens: 1000,
      total_tokens: 10000,
      active_ms: 442_467
    }
    const seeded = seedUsage(live, response, request)
    expect(seeded.usage?.samples).toBe(9)
    expect(seeded.usage?.promptTokens).toBe(9000)
    expect(seeded.usage?.activeMs).toBe(442_467)
    expect(seeded.usage?.lastPromptTokens).toBeNull()

    const fromServer = seedUsage({ ...emptyItemsState(), usageRequest: request }, response, request)
    expect(fromServer.usage).toEqual({
      samples: 9,
      promptTokens: 9000,
      completionTokens: 1000,
      totalTokens: 10000,
      costUsd: null,
      cacheHitTokens: null,
      cacheMissTokens: null,
      cacheCountersIncomplete: true,
      cacheHitRateApproximate: false,
      durationMs: null,
      durationIncomplete: true,
      activeMs: 442_467,
      lastPromptTokens: null,
      lastRoundTokensPerSec: null
    })

    // Re-seeding the same snapshot is idempotent; it must not append the live
    // sample a second time merely because another terminal refresh completed.
    const backfilled = seedUsage(seeded, response, request)
    expect(backfilled.usage?.samples).toBe(9)
    expect(backfilled.usage?.promptTokens).toBe(9000)
    expect(backfilled.usage?.activeMs).toBe(442_467)
  })
})

describe('items reducer active clock', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'

  it('anchors a mid-turn snapshot at apply time and resets the streaming window', () => {
    const snapshotAt = Date.parse('2026-10-28T00:00:30Z')
    const state = seedFromItems([], 8, {
      active_turn: {
        turn_id: TURN_ID,
        thread_id: THREAD_ID,
        state: 'running',
        started_at: '2026-10-28T00:00:00Z'
      },
      pending_approvals: [],
      pending_user_inputs: []
    }, snapshotAt)
    expect(state.rounds).toEqual([{ turnId: TURN_ID, durationMs: 0, activeMs: 0 }])
    expect(state.turnLastEventAt[TURN_ID]).toBe(snapshotAt)
    expect(state.roundWindow).toBeNull()
  })

  it('preserves the active clock and round window when the same turn is reseeded mid-turn', () => {
    const runtime = {
      active_turn: {
        turn_id: TURN_ID,
        thread_id: THREAD_ID,
        state: 'running' as const,
        started_at: '2026-10-28T00:00:00Z'
      },
      pending_approvals: [],
      pending_user_inputs: []
    }
    let state = seedFromItems([], 8, runtime, Date.parse('2026-10-28T00:00:30Z'))
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 9, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:31Z', delta: 'a'
    })
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 10, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:32Z', delta: 'b'
    })
    const rounds = state.rounds
    const window = state.roundWindow
    const anchors = state.turnLastEventAt

    const reseeded = seedFromItems([], state.lastSeq, runtime, Date.parse('2026-10-28T00:00:35Z'), state)

    expect(reseeded.activeTurnId).toBe(TURN_ID)
    expect(reseeded.rounds).toBe(rounds)
    expect(reseeded.rounds[0]?.activeMs).toBe(2000)
    expect(reseeded.turnLastEventAt).toBe(anchors)
    expect(reseeded.turnLastEventAt[TURN_ID]).toBe(Date.parse('2026-10-28T00:00:32Z'))
    expect(reseeded.roundWindow).toBe(window)
  })

  it('does not charge a mid-turn first round for time before the snapshot', () => {
    const snapshotAt = Date.parse('2026-10-28T00:00:30Z')
    let state = seedFromItems([], 8, {
      active_turn: {
        turn_id: TURN_ID,
        thread_id: THREAD_ID,
        state: 'running',
        started_at: '2026-10-28T00:00:00Z'
      },
      pending_approvals: [],
      pending_user_inputs: []
    }, snapshotAt)
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 9, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:31Z', delta: 'a'
    })
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 10, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:32Z', delta: 'b'
    })
    state = applyEvent(state, {
      kind: 'usage', seq: 11, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:32.500Z',
      usage: { prompt_tokens: 100, completion_tokens: 50, total_tokens: 150 }
    })

    expect(state.usage?.lastRoundTokensPerSec).toBeCloseTo(50)
    expect(state.rounds[0]?.durationMs).toBe(1000)
    expect(state.rounds[0]?.activeMs).toBe(2500)
  })

  it('does not scan historical round rows for each streamed token', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_started', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:00Z', auto_continuation: false,
      auto_recovery: 0, attachment_ids: [], attachment_outcomes: []
    })
    const rounds = new Proxy(state.rounds, {
      get(target, property, receiver) {
        if (property === 'some' || property === 'find') throw new Error('round scan')
        return Reflect.get(target, property, receiver)
      }
    })
    state = { ...state, rounds, turnLastEventAt: {} }

    expect(() => applyEvent(state, {
      kind: 'assistant_text_delta', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:01Z', delta: 'x'
    })).not.toThrow()
  })

  it('does not enumerate historical gates for each streamed token', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_started', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:00Z', auto_continuation: false,
      auto_recovery: 0, attachment_ids: [], attachment_outcomes: []
    })
    state = applyEvent(state, {
      kind: 'approval_requested', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', tool: 'bash', summary: 'run',
      timestamp: '2026-10-28T00:00:01Z'
    })
    const approvals = new Proxy(state.approvals, {
      ownKeys() { throw new Error('gate scan') }
    })
    state = { ...state, approvals }

    expect(() => applyEvent(state, {
      kind: 'assistant_text_delta', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:02Z', delta: 'x'
    })).not.toThrow()
  })

  it('maintains the round lookup when turns start and live usage arrives', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_started', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:00Z', auto_continuation: false,
      auto_recovery: 0, attachment_ids: [], attachment_outcomes: []
    })
    expect(state.roundIndex[TURN_ID]).toBe(0)
    state = applyEvent(state, {
      kind: 'usage', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:01Z',
      usage: { prompt_tokens: 10, completion_tokens: 2, total_tokens: 12 }
    })
    expect(state.roundIndex[TURN_ID]).toBe(0)
    expect(state.rounds).toHaveLength(1)
  })

  it('counts pending gates once across duplicate requests and resolutions', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'approval_requested', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', tool: 'bash', summary: 'run',
      timestamp: '2026-10-28T00:00:00Z'
    })
    state = applyEvent(state, {
      kind: 'approval_requested', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', tool: 'bash', summary: 'run',
      timestamp: '2026-10-28T00:00:01Z'
    })
    expect(state.pendingGateCounts[TURN_ID]).toBe(1)
    state = markApprovalLocally(state, 'approval-1', true)
    expect(state.pendingGateCounts[TURN_ID]).toBe(1)
    state = applyEvent(state, {
      kind: 'approval_resolved', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', approved: true, timestamp: '2026-10-28T00:00:02Z'
    })
    expect(state.pendingGateCounts[TURN_ID]).toBeUndefined()
  })

  it('banks elapsed time before moving the anchor and settles it on terminal', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_started', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:00Z', auto_continuation: false,
      auto_recovery: 0, attachment_ids: [], attachment_outcomes: []
    })
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:10Z', delta: 'x'
    })
    expect(state.rounds[0]?.activeMs).toBe(10_000)
    state = applyEvent(state, {
      kind: 'turn_completed', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:20Z'
    })
    expect(state.rounds[0]?.activeMs).toBe(20_000)
    expect(state.turnState[TURN_ID]).toBe('completed')
  })
})

describe('items reducer answered user input', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'

  it('materializes one durable completed row when resolution wins the item snapshot race', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'user_input_requested', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000f1',
      request_id: 'input-1', prompt: 'pick a branch', timestamp: '2026-10-28T00:00:00Z'
    })
    state = applyEvent(state, {
      kind: 'user_input_resolved', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000f1',
      request_id: 'input-1', answer: 'main', timestamp: '2026-10-28T00:00:01Z'
    })
    expect(state.itemOrder).toHaveLength(1)
    const row = state.itemsById[state.itemOrder[0]!]
    expect(row?.kind).toBe('user_input')
    expect(row?.status).toBe('completed')
    expect(row?.text).toBe('pick a branch')
    expect(row?.detail).toBe('main')

    const replayed = applyEvent(state, {
      kind: 'item_created', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:01Z',
      item: { ...row!, item_id: row!.item_id }
    })
    expect(replayed.itemOrder).toEqual(state.itemOrder)
  })
})

describe('items reducer child activity count', () => {
  const base = { thread_id: '00000000-0000-0000-0000-000000000001', timestamp: '2026-10-28T00:00:00Z' }

  it('counts unique active children across queued/running and repeated terminal events', () => {
    let state = emptyItemsState()
    const activity = (seq: number, child_id: string, status: 'queued' | 'running' | 'completed'): RuntimeEvent => ({
      kind: 'child_activity', ...base, seq, child_id, label: child_id, status
    })
    state = applyEvent(state, activity(1, 'child-a', 'queued'))
    state = applyEvent(state, activity(2, 'child-a', 'running'))
    expect(state.childRunActiveCount).toBe(1)
    state = applyEvent(state, activity(3, 'child-b', 'running'))
    expect(state.childRunActiveCount).toBe(2)
    state = applyEvent(state, activity(4, 'child-a', 'completed'))
    state = applyEvent(state, activity(5, 'child-a', 'completed'))
    expect(state.childRunActiveCount).toBe(1)
  })
})

/**
 * Workbench panels subscribe to `workbenchRevision` instead of `lastSeq`.
 * Only events the server workbench projections actually consume may advance it,
 * or every streamed token would trigger a full HTTP reload of all four panels.
 */
describe('items reducer workbench revision', () => {
  const THREAD = '00000000-0000-0000-0000-000000000001'
  const TURN = '00000000-0000-0000-0000-000000000003'
  const ITEM = '00000000-0000-0000-0000-000000000004'
  const base = { seq: 1, thread_id: THREAD, timestamp: '2026-10-28T00:00:00Z' }

  it('does not advance for streamed text tokens, usage or child lifecycle', () => {
    let state = emptyItemsState()
    state = applyEvent(state, { kind: 'assistant_text_delta', ...base, turn_id: TURN, item_id: ITEM, delta: 'x' })
    state = applyEvent(state, { kind: 'usage', ...base, seq: 2, turn_id: TURN, usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } })
    state = applyEvent(state, { kind: 'child_activity', ...base, seq: 3, child_id: '00000000-0000-0000-0000-000000000002', label: 'c', status: 'running' })

    expect(state.lastSeq).toBe(3)
    expect(state.workbenchRevision).toBe(0)
  })

  it('advances for every event the server workbench projections consume', () => {
    let state = emptyItemsState()
    state = applyEvent(state, { kind: 'goal_updated', ...base, goal: 'ship' })
    state = applyEvent(state, { kind: 'todos_updated', ...base, seq: 2, todos: [] })
    state = applyEvent(state, { kind: 'tool_call_started', ...base, seq: 3, turn_id: TURN, item_id: ITEM, call_id: 'c1', tool: 'write', arguments: {} })
    state = applyEvent(state, { kind: 'tool_call_finished', ...base, seq: 4, turn_id: TURN, item_id: ITEM, call_id: 'c1', tool: 'write', ok: true, duration_ms: 1, output: '' })

    expect(state.workbenchRevision).toBe(4)
  })

  it('folds goal/todo updates and clears into the renderer plan projection', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    try {
      let state = applyEvent(emptyItemsState(), { kind: 'goal_updated', ...base, goal: 'ship' })
      state = applyEvent(state, {
        kind: 'todos_updated', ...base, seq: 2,
        todos: [{ id: 'todo-1', title: 'write tests', status: 'in_progress' }]
      })
      expect(state.plan).toEqual({
        goal: 'ship',
        todos: [{ id: 'todo-1', title: 'write tests', status: 'in_progress' }]
      })
      state = applyEvent(state, { kind: 'goal_cleared', ...base, seq: 3 })
      state = applyEvent(state, { kind: 'todos_cleared', ...base, seq: 4 })
      expect(state.plan).toEqual({ todos: [] })
      expect(state.workbenchRevision).toBe(4)
      expect(warn).not.toHaveBeenCalled()
    } finally {
      warn.mockRestore()
    }
  })

  it('bumps monotonically on rewind so a rebuilt snapshot is never stale', () => {
    let state = emptyItemsState()
    state = applyEvent(state, { kind: 'goal_updated', ...base, goal: 'ship' })
    state = applyEvent(state, { kind: 'thread_rewound', ...base, seq: 9, rewind_from_seq: 3, target_turn_id: TURN })

    expect(state.workbenchRevision).toBe(2)
    expect(state.lastSeq).toBe(9)
  })

  it('重放的旧 seq 增量不会把文本追加两次', () => {
    const seeded = seedFromItems(
      [{
        item_id: ITEM,
        turn_id: TURN,
        kind: 'assistant_message',
        status: 'streaming',
        created_at: '2026-10-28T00:00:00Z',
        text: 'a'
      }],
      5,
      { pending_approvals: [], pending_user_inputs: [] }
    )
    const replayed = applyEvent(seeded, {
      kind: 'assistant_text_delta',
      ...base,
      seq: 3,
      turn_id: TURN,
      item_id: ITEM,
      delta: 'b'
    })

    expect(replayed).toBe(seeded)
    expect(replayed.itemsById[ITEM]?.text).toBe('a')
  })

  it('重放的 usage 事件不会重复累加', () => {
    const seeded = seedFromItems([], 5, { pending_approvals: [], pending_user_inputs: [] })
    const replayed = applyEvent(seeded, {
      kind: 'usage',
      ...base,
      seq: 2,
      turn_id: TURN,
      usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 }
    })

    expect(replayed).toBe(seeded)
    expect(replayed.usage).toBeNull()
  })
})

/**
 * A terminal failure renders inline at the interruption point. The server-side
 * projector folds `turn_failed` into a durable `error` item stored in
 * `items.jsonl`, but the live SSE channel only carries the bare event, so the
 * reducer materializes the same row with the server's deterministic id.
 */
describe('items reducer turn_failed', () => {
  const THREAD_ID = '00000000-0000-0000-0000-000000000001'
  const TURN_ID = '00000000-0000-0000-0000-000000000003'

  it('live 折叠出与 Rust 投影同 id 的中断处错误行', () => {
    let state = emptyItemsState()
    state = applyEvent(state, {
      kind: 'turn_started',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:00Z',
      auto_continuation: false,
      auto_recovery: 0,
      attachment_ids: [],
      attachment_outcomes: []
    })
    state = applyEvent(state, {
      kind: 'turn_failed',
      seq: 2,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000e1',
      timestamp: '2026-10-28T00:00:01Z',
      message: 'turn budget: max model rounds reached',
      model_request_failed: false
    })

    expect(state.turnState[TURN_ID]).toBe('failed')
    expect(state.activeTurnId).toBeNull()
    expect(state.itemOrder).toHaveLength(1)
    const row = state.itemsById[state.itemOrder[0]!]
    expect(row?.kind).toBe('error')
    expect(row?.status).toBe('failed')
    expect(row?.turn_id).toBe(TURN_ID)
    expect(row?.text).toBe('turn budget: max model rounds reached')
  })

  it('直接使用 Rust 事件提供的 turn_failed item id', () => {
    const itemId = '00000000-0000-0000-0000-000000000099'
    const state = applyEvent(emptyItemsState(), {
      kind: 'turn_failed',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      item_id: itemId,
      timestamp: '2026-10-28T00:00:01Z',
      message: 'x',
      model_request_failed: false
    })
    expect(state.itemOrder[0]).toBe(itemId)
  })

  it('刷新 seed 携带同一 item 时合并为一行，不重复', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_failed',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000e1',
      timestamp: '2026-10-28T00:00:01Z',
      message: 'x',
      model_request_failed: false
    })
    state = applyEvent(state, {
      kind: 'item_created',
      seq: 2,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:01Z',
      item: {
        item_id: state.itemOrder[0]!,
        turn_id: TURN_ID,
        kind: 'error',
        status: 'failed',
        created_at: '2026-10-28T00:00:01Z',
        text: 'x'
      }
    })
    expect(state.itemOrder).toHaveLength(1)
  })

  it('服务端投影的 error item 照常落到行投影，展示在中断处', () => {
    const state = applyEvent(emptyItemsState(), {
      kind: 'item_created',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:01Z',
      item: {
        item_id: '00000000-0000-0000-0000-000000000004',
        turn_id: TURN_ID,
        kind: 'error',
        status: 'failed',
        created_at: '2026-10-28T00:00:01Z',
        text: 'turn budget: max model rounds reached'
      }
    })

    expect(state.itemOrder).toEqual(['00000000-0000-0000-0000-000000000004'])
    expect(state.itemsById['00000000-0000-0000-0000-000000000004']?.kind).toBe('error')
    expect(state.itemsById['00000000-0000-0000-0000-000000000004']?.status).toBe('failed')
  })
})

describe('live tool_call fold (实时回合工具行投影)', () => {
  const base = {
    seq: 1,
    thread_id: '00000000-0000-0000-0000-000000000000' as const,
    timestamp: '2026-11-16T01:00:00Z',
    auto_continuation: false,
    auto_recovery: 0,
    attachment_ids: [] as string[],
    attachment_outcomes: []
  }
  const turnId = '00000000-0000-0000-0000-0000000000t1' as const

  it('tool_call_started/finished 折叠出与服务器同 id 的 write 行(含 arguments)', () => {
    let state = applyEvent(emptyItemsState(), {
      ...base, kind: 'turn_started', turn_id: turnId
    })
    state = applyEvent(state, {
      ...base, seq: 2, kind: 'tool_call_started', turn_id: turnId,
      item_id: '00000000-0000-0000-0000-0000000000a1',
      call_id: 'c1', tool: 'write', arguments: { path: 'a.md', content: 'x' }
    })
    state = applyEvent(state, {
      ...base, seq: 3, kind: 'tool_call_finished', turn_id: turnId,
      item_id: '00000000-0000-0000-0000-0000000000a1',
      call_id: 'c1', tool: 'write', ok: true, duration_ms: 5, output: 'wrote 1 byte'
    })
    const rows = state.itemOrder.map((id) => state.itemsById[id]!)
    const write = rows.find((r) => r.tool?.tool === 'write')
    expect(write?.item_id).toBe('00000000-0000-0000-0000-0000000000a1')
    expect(write).toBeTruthy()
    expect(write!.status).toBe('completed')
    expect(write!.tool!.ok).toBe(true)
    expect((write!.tool!.arguments as { path: string }).path).toBe('a.md')
  })

  it('uses the item_id supplied by Rust instead of recomputing it', () => {
    const itemId = '00000000-0000-0000-0000-0000000000aa' as const
    const state = applyEvent(emptyItemsState(), {
      ...base,
      kind: 'tool_call_started',
      turn_id: turnId,
      item_id: itemId,
      call_id: 'server-keyed',
      tool: 'read',
      arguments: {}
    } as RuntimeEvent)

    expect(state.itemOrder).toEqual([itemId])
    expect(state.itemsById[itemId]?.tool?.call_id).toBe('server-keyed')
  })

  it('finished 先于 started 到达(重连窗口)也能建行', () => {
    let state = applyEvent(emptyItemsState(), {
      ...base, seq: 2, kind: 'tool_call_finished', turn_id: turnId,
      item_id: '00000000-0000-0000-0000-0000000000a2',
      call_id: 'c2', tool: 'bash', ok: false, duration_ms: 1, output: '', detail: 'cancelled'
    })
    const row = state.itemOrder.map((id) => state.itemsById[id]!).find((r) => r.tool?.call_id === 'c2')
    expect(row).toBeTruthy()
    expect(row!.tool!.ok).toBe(false)
  })
})
