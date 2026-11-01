import { describe, expect, it } from 'vitest'
import type { RuntimeEvent } from '../../../shared/contracts'
import { childCompletionStatusKey } from '../child-completion-status'
import { applyEvent, emptyItemsState, seedFromItems } from './items-reducer'

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
  it('keeps only the newest fifty distinct completion cards', () => {
    let state = emptyItemsState()
    for (let seq = 1; seq <= 51; seq += 1) state = applyEvent(state, childCompleted(seq))

    expect(Object.keys(state.childCompletions)).toHaveLength(50)
    expect(state.childCompletions['child:1']).toBeUndefined()
    expect(state.childCompletions['child:51']?.seq).toBe(51)
  })

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
    const state = seedFromItems([], 8, [], {
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
      [{
        notification_id: 'child:1',
        child_id: '00000000-0000-0000-0000-000000000002',
        thread_id: THREAD_ID,
        label: 'inspect runtime',
        status: 'completed',
        summary: 'healthy',
        completed_at: '2026-10-28T00:00:01Z',
        seq: 5
      }],
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
    expect(state.childCompletions['child:1']).toBeUndefined()
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
    const rebuilt = seedFromItems([], afterRewind.lastSeq, [], {
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
      [],
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
    expect(next.errors).toBe(start.errors)
    expect(next.childCompletions).toBe(start.childCompletions)
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
