import { describe, expect, it } from 'vitest'
import type { RuntimeEvent } from '../../../shared/contracts'
import { childCompletionStatus } from '../child-completion-status'
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

  it('renders terminal child status in Chinese', () => {
    expect(childCompletionStatus('queued')).toBe('排队中')
    expect(childCompletionStatus('running')).toBe('执行中')
    expect(childCompletionStatus('completed')).toBe('已完成')
    expect(childCompletionStatus('failed')).toBe('失败')
    expect(childCompletionStatus('aborted')).toBe('已中止')
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
