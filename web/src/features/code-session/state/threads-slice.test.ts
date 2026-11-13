import { beforeEach, describe, expect, it } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { useCodeSessionStore } from './store'

const A = '00000000-0000-0000-0000-00000000000a'
const UNKNOWN = '00000000-0000-0000-0000-00000000000b'

function row(overrides: Partial<ThreadSummary> = {}): ThreadSummary {
  return {
    thread_id: A,
    title: 'a',
    created_at: '2026-10-28T00:00:00Z',
    updated_at: '2026-10-28T00:00:00Z',
    archived: false,
    last_seq: 3,
    ...overrides
  }
}

describe('threads slice noteThreadSeq', () => {
  beforeEach(() => {
    useCodeSessionStore.setState({ threads: [row()], activeThreadId: null })
  })

  it('推进列表水位但不改变行顺序与其它字段', () => {
    useCodeSessionStore.getState().noteThreadSeq(A, 7)

    const threads = useCodeSessionStore.getState().threads
    expect(threads).toHaveLength(1)
    expect(threads[0]?.last_seq).toBe(7)
    expect(threads[0]?.title).toBe('a')
  })

  it('忽略过期或重复的水位，不替换数组引用', () => {
    const before = useCodeSessionStore.getState().threads
    useCodeSessionStore.getState().noteThreadSeq(A, 2)
    useCodeSessionStore.getState().noteThreadSeq(A, 3)

    expect(useCodeSessionStore.getState().threads).toBe(before)
  })

  it('忽略未知会话', () => {
    useCodeSessionStore.getState().noteThreadSeq(UNKNOWN, 9)

    expect(useCodeSessionStore.getState().threads[0]?.last_seq).toBe(3)
  })
})
