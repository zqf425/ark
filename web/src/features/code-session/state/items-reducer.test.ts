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
