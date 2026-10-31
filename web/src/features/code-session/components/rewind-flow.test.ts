import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '../../../shared/api/client'
import { canRewindTurn, executeRewind } from './rewind-flow'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_ID = '00000000-0000-0000-0000-0000000000aa'

function apiError(status: number): ApiError {
  return new ApiError({ status, code: 'unknown', message: 'boom' })
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe('canRewindTurn', () => {
  it('仅终态回合可回退', () => {
    expect(canRewindTurn('completed')).toBe(true)
    expect(canRewindTurn('failed')).toBe(true)
    expect(canRewindTurn('aborted')).toBe(true)
    expect(canRewindTurn('running')).toBe(false)
    expect(canRewindTurn('aborting')).toBe(false)
    expect(canRewindTurn('idle')).toBe(false)
    expect(canRewindTurn(undefined)).toBe(false)
  })
})

describe('executeRewind', () => {
  it('成功时以 turn_id 调用 API 并回报新的 last_seq', async () => {
    const rewind = vi.fn().mockResolvedValue({ last_seq: 41, rewind_from_seq: 12, already_rewound: false })
    const onRewound = vi.fn()
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_ID, activeTurnId: null, running: false },
      { rewind, onRewound, onError }
    )

    expect(ok).toBe(true)
    expect(rewind).toHaveBeenCalledWith(THREAD_ID, { turn_id: TURN_ID })
    expect(onRewound).toHaveBeenCalledWith(41)
    expect(onError).not.toHaveBeenCalled()
  })

  it('线程仍有运行中回合时不发起请求', async () => {
    const rewind = vi.fn()
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_ID, activeTurnId: 'other-turn', running: true },
      { rewind, onRewound: vi.fn(), onError }
    )

    expect(ok).toBe(false)
    expect(rewind).not.toHaveBeenCalled()
    expect(onError).not.toHaveBeenCalled()
  })

  it('409 冲突走错误通道且不触发重载', async () => {
    const rewind = vi.fn().mockRejectedValue(apiError(409))
    const onError = vi.fn()
    const onRewound = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_ID, activeTurnId: null, running: false },
      { rewind, onRewound, onError }
    )

    expect(ok).toBe(false)
    expect(onRewound).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledTimes(1)
    expect(onError.mock.calls[0]?.[0]).toBeInstanceOf(ApiError)
    expect((onError.mock.calls[0]?.[0] as ApiError).status).toBe(409)
  })

  it('404 目标不存在走错误通道且不触发重载', async () => {
    const rewind = vi.fn().mockRejectedValue(apiError(404))
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_ID, activeTurnId: null, running: false },
      { rewind, onRewound: vi.fn(), onError }
    )

    expect(ok).toBe(false)
    expect(onError).toHaveBeenCalledTimes(1)
    expect(onError.mock.calls[0]?.[0]).toBeInstanceOf(ApiError)
    expect((onError.mock.calls[0]?.[0] as ApiError).status).toBe(404)
  })
})
