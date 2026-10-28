import { describe, expect, it } from 'vitest'
import type { RuntimeEvent } from '../../../shared/contracts'
import { childCompletionStatus } from '../child-completion-status'
import { applyEvent, emptyItemsState } from './items-reducer'

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
    expect(childCompletionStatus('completed')).toBe('已完成')
    expect(childCompletionStatus('failed')).toBe('失败')
    expect(childCompletionStatus('aborted')).toBe('已中止')
  })
})
