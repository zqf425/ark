import { describe, expect, it } from 'vitest'
import { applyEvent, emptyItemsState, markApprovalLocally, markUserInputLocally } from './items-reducer'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_ID = '00000000-0000-0000-0000-000000000003'

function startedTurn() {
  return applyEvent(emptyItemsState(), {
    kind: 'turn_started', seq: 1, thread_id: THREAD_ID, turn_id: TURN_ID,
    timestamp: '2026-10-28T00:00:00Z', auto_continuation: false,
    auto_recovery: 0, attachment_ids: [], attachment_outcomes: []
  })
}

describe('items reducer gate clock', () => {
  it('keeps approval wait excluded until the durable resolution arrives', () => {
    let state = startedTurn()
    state = applyEvent(state, {
      kind: 'approval_requested', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', tool: 'bash', summary: 'run',
      timestamp: '2026-10-28T00:00:01Z'
    })
    state = markApprovalLocally(state, 'approval-1', true)
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:11Z', delta: 'x'
    })
    expect(state.rounds[0]?.activeMs).toBe(1_000)

    state = applyEvent(state, {
      kind: 'approval_resolved', seq: 4, thread_id: THREAD_ID, turn_id: TURN_ID,
      approval_id: 'approval-1', approved: true, timestamp: '2026-10-28T00:00:12Z'
    })
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 5, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:14Z', delta: 'y'
    })
    expect(state.rounds[0]?.activeMs).toBe(3_000)
  })

  it('keeps user-input wait excluded until the durable resolution arrives', () => {
    let state = startedTurn()
    state = applyEvent(state, {
      kind: 'user_input_requested', seq: 2, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000f1',
      request_id: 'input-1', prompt: 'pick a branch', timestamp: '2026-10-28T00:00:01Z'
    })
    state = markUserInputLocally(state, 'input-1', 'main')
    state = applyEvent(state, {
      kind: 'user_input_resolved', seq: 3, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-0000000000f1',
      request_id: 'input-1', answer: 'main', timestamp: '2026-10-28T00:00:12Z'
    })
    state = applyEvent(state, {
      kind: 'assistant_text_delta', seq: 4, thread_id: THREAD_ID, turn_id: TURN_ID,
      item_id: '00000000-0000-0000-0000-000000000004',
      timestamp: '2026-10-28T00:00:14Z', delta: 'x'
    })
    expect(state.rounds[0]?.activeMs).toBe(3_000)
  })
})
