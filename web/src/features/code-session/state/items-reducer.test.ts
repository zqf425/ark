import { describe, expect, it } from 'vitest'
import type { RuntimeEvent } from '../../../shared/contracts'
import { childCompletionStatusKey } from '../child-completion-status'
import { applyEvent, emptyItemsState, seedFromItems } from './items-reducer'
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

  it('流式增量只替换 itemsById，其余通道保持原引用', () => {
    const start = seeded()
    const next = applyEvent(start, delta(3, 'b'))

    expect(next.itemsById).not.toBe(start.itemsById)
    expect(next.itemOrder).toBe(start.itemOrder)
    expect(next.turnState).toBe(start.turnState)
    expect(next.approvals).toBe(start.approvals)
    expect(next.userInputs).toBe(start.userInputs)
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
      // Neither sample carried a measured duration: unknown, not `0秒`.
      durationMs: null,
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

  it('seedUsage 只填充空通道，不覆盖实时事件累加结果', () => {
    const live = applyEvent(emptyItemsState(), usageEvent(1, 1000, 200))
    const seeded = seedUsage(live, {
      thread_id: THREAD_ID,
      turns: 3,
      samples: 9,
      turn_usage: [],
      prompt_tokens: 9000,
      completion_tokens: 1000,
      total_tokens: 10000
    })
    expect(seeded.usage).toBe(live.usage)

    const fromServer = seedUsage(emptyItemsState(), {
      thread_id: THREAD_ID,
      turns: 3,
      samples: 9,
      turn_usage: [],
      prompt_tokens: 9000,
      completion_tokens: 1000,
      total_tokens: 10000
    })
    expect(fromServer.usage).toEqual({
      samples: 9,
      promptTokens: 9000,
      completionTokens: 1000,
      totalTokens: 10000,
      costUsd: null,
      cacheHitTokens: null,
      cacheMissTokens: null,
      // The lifetime seed reports no spanning total here, so nothing is faked.
      durationMs: null,
      activeMs: null,
      lastPromptTokens: null,
      lastRoundTokensPerSec: null
    })
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
    state = applyEvent(state, { kind: 'tool_call_started', ...base, seq: 3, turn_id: TURN, call_id: 'c1', tool: 'write', arguments: {} })
    state = applyEvent(state, { kind: 'tool_call_finished', ...base, seq: 4, turn_id: TURN, call_id: 'c1', tool: 'write', ok: true, duration_ms: 1, output: '' })

    expect(state.workbenchRevision).toBe(4)
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

  it('live 折叠 id 与 Rust keyed_item_id("turn_failed:{turn}") 完全一致', () => {
    // Expected value precomputed with the Rust algorithm (fnv1a hi/lo seeded
    // with the offset-basis pairs, custom-variant bits forced) for
    // `turn_failed:00000000-0000-0000-0000-000000000003`.
    const state = applyEvent(emptyItemsState(), {
      kind: 'turn_failed',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
      timestamp: '2026-10-28T00:00:01Z',
      message: 'x',
      model_request_failed: false
    })
    expect(state.itemOrder[0]).toBe('47bfd081-5592-8d05-a602-cad4686b2174')
  })

  it('刷新 seed 携带同一 item 时合并为一行，不重复', () => {
    let state = applyEvent(emptyItemsState(), {
      kind: 'turn_failed',
      seq: 1,
      thread_id: THREAD_ID,
      turn_id: TURN_ID,
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
      call_id: 'c1', tool: 'write', arguments: { path: 'a.md', content: 'x' }
    })
    state = applyEvent(state, {
      ...base, seq: 3, kind: 'tool_call_finished', turn_id: turnId,
      call_id: 'c1', tool: 'write', ok: true, duration_ms: 5, output: 'wrote 1 byte'
    })
    const rows = state.itemOrder.map((id) => state.itemsById[id]!)
    const write = rows.find((r) => r.tool?.tool === 'write')
    expect(write).toBeTruthy()
    expect(write!.status).toBe('completed')
    expect(write!.tool!.ok).toBe(true)
    expect((write!.tool!.arguments as { path: string }).path).toBe('a.md')
  })

  it('finished 先于 started 到达(重连窗口)也能建行', () => {
    let state = applyEvent(emptyItemsState(), {
      ...base, seq: 2, kind: 'tool_call_finished', turn_id: turnId,
      call_id: 'c2', tool: 'bash', ok: false, duration_ms: 1, output: '', detail: 'cancelled'
    })
    const row = state.itemOrder.map((id) => state.itemsById[id]!).find((r) => r.tool?.call_id === 'c2')
    expect(row).toBeTruthy()
    expect(row!.tool!.ok).toBe(false)
  })
})
