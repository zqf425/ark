import { beforeEach, describe, expect, it } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { useCodeSessionStore } from './store'

const A = '00000000-0000-0000-0000-00000000000a'
const B = '00000000-0000-0000-0000-00000000000b'
const UNKNOWN = '00000000-0000-0000-0000-00000000000c'

function row(overrides: Partial<ThreadSummary> = {}): ThreadSummary {
  return {
    thread_id: A,
    title: 'a',
    created_at: '2026-10-28T00:00:00Z',
    updated_at: '2026-10-28T00:00:00Z',
    archived: false,
    running: false,
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

describe('threads slice activity ordering', () => {
  beforeEach(() => {
    useCodeSessionStore.setState({ threads: [], activeThreadId: null })
  })

  it('noteThreadActivity 把正在流式输出的会话顶到列表最前', () => {
    useCodeSessionStore.getState().setThreads([
      row({ thread_id: A, title: '旧活动' }),
      row({ thread_id: B, title: '新活动', updated_at: '2026-10-29T00:00:00Z' })
    ])
    expect(useCodeSessionStore.getState().threads.map((t) => t.title)).toEqual(['新活动', '旧活动'])

    useCodeSessionStore.getState().noteThreadActivity(A, '2026-10-30T00:00:00Z')

    expect(useCodeSessionStore.getState().threads.map((t) => t.title)).toEqual(['旧活动', '新活动'])
  })

  it('noteThreadActivity 忽略更旧的时间戳（乱序/重复帧不回退排序）', () => {
    useCodeSessionStore.getState().setThreads([
      row({ thread_id: A, last_event_at: '2026-10-30T00:00:00Z' })
    ])
    const before = useCodeSessionStore.getState().threads

    useCodeSessionStore.getState().noteThreadActivity(A, '2026-10-29T00:00:00Z')

    expect(useCodeSessionStore.getState().threads).toBe(before)
  })

  it('applyRunningFlags 合并轮询结果：在集合中为忙碌，缺席为空闲', () => {
    useCodeSessionStore.getState().setThreads([
      row({ thread_id: A, running: false }),
      row({ thread_id: B, title: 'b' })
    ])

    useCodeSessionStore.getState().applyRunningFlags([B])

    const threads = useCodeSessionStore.getState().threads
    expect(threads.find((t) => t.thread_id === A)?.running).toBe(false)
    expect(threads.find((t) => t.thread_id === B)?.running).toBe(true)
  })

  it('setThreadRunning 只改目标行的忙碌标记', () => {
    useCodeSessionStore.getState().setThreads([
      row({ thread_id: A }),
      row({ thread_id: B, title: 'b' })
    ])

    useCodeSessionStore.getState().setThreadRunning(A, true)
    useCodeSessionStore.getState().setThreadRunning(A, true)

    const threads = useCodeSessionStore.getState().threads
    expect(threads.find((t) => t.thread_id === A)?.running).toBe(true)
    expect(threads.find((t) => t.thread_id === B)?.running).toBe(false)
  })
})

describe('threads slice renameThreadRow', () => {
  beforeEach(() => {
    useCodeSessionStore.setState({ threads: [row()], activeThreadId: null })
  })

  it('更新标题但不改变行序、水位或其它字段', () => {
    const before = useCodeSessionStore.getState().threads[0]
    useCodeSessionStore.getState().renameThreadRow(A, '自动标题')

    const row0 = useCodeSessionStore.getState().threads[0]
    expect(row0?.title).toBe('自动标题')
    expect(row0?.last_seq).toBe(3)
    expect(row0?.thread_id).toBe(before?.thread_id)
    expect(useCodeSessionStore.getState().threads).toHaveLength(1)
  })

  it('忽略未知会话与空标题', () => {
    const before = useCodeSessionStore.getState().threads
    useCodeSessionStore.getState().renameThreadRow(UNKNOWN, 'x')
    useCodeSessionStore.getState().renameThreadRow(A, '   ')

    expect(useCodeSessionStore.getState().threads).toBe(before)
  })
})
