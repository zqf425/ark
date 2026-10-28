import { describe, expect, it } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { filterThreads } from './thread-list-filter'

const threads: ThreadSummary[] = [
  {
    thread_id: '00000000-0000-0000-0000-000000000001',
    title: '梳理 provider 重试策略',
    created_at: '2026-10-28T00:00:00Z',
    updated_at: '2026-10-28T00:00:00Z',
    archived: false
  },
  {
    thread_id: '00000000-0000-0000-0000-000000000002',
    title: '旧的 MCP 方案',
    created_at: '2026-10-27T00:00:00Z',
    updated_at: '2026-10-27T00:00:00Z',
    archived: true
  }
]

describe('thread list filtering', () => {
  it('keeps recent and archived conversations separate while searching the loaded list', () => {
    expect(filterThreads(threads, 'PROVIDER', 'recent').map((thread) => thread.title)).toEqual(['梳理 provider 重试策略'])
    expect(filterThreads(threads, '', 'archived').map((thread) => thread.title)).toEqual(['旧的 MCP 方案'])
  })
})
