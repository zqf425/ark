import { describe, expect, it } from 'vitest'
import type { ChildRunSummary } from '../../../shared/contracts'
import { childRunMeta } from './child-run-meta'

const child = (overrides: Partial<ChildRunSummary> = {}): ChildRunSummary => ({
  id: '00000000-0000-0000-0000-000000000001',
  parentThreadId: '00000000-0000-0000-0000-000000000002',
  parentTurnId: '00000000-0000-0000-0000-000000000003',
  label: 'inspect runtime',
  status: 'completed',
  createdAt: '2026-10-28T00:00:00Z',
  ...overrides
})

describe('childRunMeta', () => {
  it('shows only fields reported by the runtime', () => {
    expect(childRunMeta(child({ model: 'gpt-5', providerId: 'openai', toolInvocations: 3, resumeCount: 2 }))).toEqual([
      'openai · gpt-5',
      '3 次工具调用',
      '已恢复 2 次'
    ])
  })

  it('does not invent metadata absent from the child record', () => {
    expect(childRunMeta(child())).toEqual([])
  })
})
