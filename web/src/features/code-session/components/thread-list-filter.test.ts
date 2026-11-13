import { describe, expect, it } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { filterThreads, filterThreadsWithWorkspace } from './thread-list-filter'

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

  it('searches the workspace path in the project-aware filter', () => {
    const withWorkspace: ThreadSummary[] = [
      { ...threads[0]!, workspace: '/Users/admin/Desktop/code/ark' },
      { ...threads[1]!, workspace: '/Users/admin/Desktop/code/qdc' }
    ]
    // `qdc`'s only thread is archived, so the "recent" scope hides it.
    expect(filterThreadsWithWorkspace(withWorkspace, 'QDC', 'recent')).toEqual([])
    expect(filterThreadsWithWorkspace(withWorkspace, 'QDC', 'archived').map((thread) => thread.title)).toEqual([
      '旧的 MCP 方案'
    ])
    expect(filterThreadsWithWorkspace(withWorkspace, 'ark', 'recent').map((thread) => thread.title)).toEqual([
      '梳理 provider 重试策略'
    ])
  })
})
