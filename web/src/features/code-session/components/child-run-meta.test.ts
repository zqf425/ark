import { describe, expect, it } from 'vitest'
import type { ChildRunSummary } from '../../../shared/contracts'
import { translate } from '../../../shared/i18n/messages'
import { childRunDuration, childRunMeta, formatChildRunDuration } from './child-run-meta'

const child = (overrides: Partial<ChildRunSummary> = {}): ChildRunSummary => ({
  id: '00000000-0000-0000-0000-000000000001',
  parent_thread_id: '00000000-0000-0000-0000-000000000002',
  parent_turn_id: '00000000-0000-0000-0000-000000000003',
  label: 'inspect runtime',
  status: 'completed',
  created_at: '2026-10-28T00:00:00Z',
  ...overrides
})

describe('childRunMeta', () => {
  it('shows only fields reported by the runtime', () => {
    const parts = childRunMeta(child({ model: 'gpt-5', provider_id: 'openai', tool_invocations: 3, resume_count: 2 }))

    expect(parts.map((part) => translate('zh-CN', part.key, part.params))).toEqual([
      'openai · gpt-5',
      '3 次工具调用',
      '已恢复 2 次'
    ])
  })

  it('does not invent metadata absent from the child record', () => {
    expect(childRunMeta(child())).toEqual([])
  })

  it('renders the same facts in English without falling back to Chinese', () => {
    const parts = childRunMeta(child({ model: 'gpt-5', provider_id: 'openai', tool_invocations: 3, resume_count: 2 }))

    expect(parts.map((part) => translate('en-US', part.key, part.params))).toEqual([
      'openai · gpt-5',
      '3 tool calls',
      'Resumed 2 times'
    ])
  })
})

describe('childRunDuration', () => {
  it('freezes at ended_at for a settled run', () => {
    const duration = childRunDuration(child({
      status: 'completed',
      created_at: '2026-10-28T00:00:00Z',
      ended_at: '2026-10-28T00:02:05Z'
    }))
    expect(duration).toBe(125_000)
  })

  it('measures up to now while the run is active', () => {
    const created = new Date(Date.now() - 30_000).toISOString()
    const duration = childRunDuration(child({ status: 'running', created_at: created }))
    expect(duration).not.toBeNull()
    expect(duration!).toBeGreaterThanOrEqual(29_000)
    expect(duration!).toBeLessThan(60_000)
  })

  it('returns null for malformed or inverted timestamps instead of a negative', () => {
    expect(childRunDuration(child({ created_at: 'not-a-date' }))).toBeNull()
    expect(childRunDuration(child({ created_at: '2026-10-28T01:00:00Z', ended_at: '2026-10-28T00:00:00Z' }))).toBeNull()
  })
})

describe('formatChildRunDuration', () => {
  it('renders minutes and zero-padded seconds', () => {
    expect(formatChildRunDuration(125_000)).toBe('2m 05s')
  })

  it('renders sub-minute runs as bare seconds', () => {
    expect(formatChildRunDuration(14_000)).toBe('14s')
  })
})
