import { describe, expect, it } from 'vitest'
import { approvalCards, userInputCards } from './selectors'
import { emptyItemsState } from './items-reducer'

describe('gate card selectors', () => {
  it('keeps a locally resolved approval visible as conversation history', () => {
    const state = emptyItemsState()
    state.approvals['approval-1'] = {
      approval_id: 'approval-1',
      turn_id: '00000000-0000-0000-0000-000000000001',
      tool: 'write',
      summary: 'update file',
      status: 'approved',
      requested_at: '2026-10-28T00:00:00Z'
    }

    expect(approvalCards(state)).toHaveLength(1)
    expect(approvalCards(state)[0]?.status).toBe('approved')
  })

  it('keeps an answered input visible as conversation history', () => {
    const state = emptyItemsState()
    state.userInputs['input-1'] = {
      request_id: 'input-1',
      turn_id: '00000000-0000-0000-0000-000000000001',
      prompt: 'pick a branch',
      status: 'answered',
      answer: 'main',
      requested_at: '2026-10-28T00:00:00Z'
    }

    expect(userInputCards(state)).toHaveLength(1)
    expect(userInputCards(state)[0]?.answer).toBe('main')
  })
})
