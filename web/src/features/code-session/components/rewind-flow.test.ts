import { describe, expect, it, vi } from 'vitest'
import { ApiError } from '../../../shared/api/client'
import type { ThreadItem } from '../../../shared/contracts'
import { emptyItemsState } from '../state/items-reducer'
import type { ItemsSnapshot } from '../state/types'
import { executeRewind, hasRunningTurn, rewindAvailability, rewindAvailabilityByTurn } from './rewind-flow'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const TURN_A = '00000000-0000-0000-0000-0000000000aa'
const TURN_B = '00000000-0000-0000-0000-0000000000bb'

function apiError(status: number): ApiError {
  return new ApiError({ status, code: 'unknown', message: 'boom' })
}

function item(turnId: string, suffix: string, status: ThreadItem['status'] = 'completed'): ThreadItem {
  return {
    item_id: `00000000-0000-0000-0000-0000000000${suffix}`,
    turn_id: turnId,
    kind: 'assistant_message',
    status,
    created_at: '2026-10-28T00:00:00Z',
    text: 'x'
  }
}

/** Two finished turns, as `GET /v1/threads/{id}` returns them: items only. */
function twoFinishedTurns(): ItemsSnapshot {
  const state = emptyItemsState()
  for (const [index, turnId] of [TURN_A, TURN_B].entries()) {
    const row = item(turnId, `${index}1`)
    state.itemsById[row.item_id] = row
    state.itemOrder.push(row.item_id)
  }
  return state
}

describe('rewindAvailability', () => {
  it('历史回合即使没有 turnState 条目也可回退（detail 不返回历史回合状态）', () => {
    const state = twoFinishedTurns()

    expect(state.turnState[TURN_A]).toBeUndefined()
    expect(rewindAvailability(state, TURN_A)).toBe('ready')
    expect(rewindAvailability(state, TURN_B)).toBe('ready')
  })

  it('运行中的回合显示为禁用而不是消失', () => {
    const state = twoFinishedTurns()
    const live = item(TURN_A, '20', 'streaming')
    state.itemsById[live.item_id] = live
    state.itemOrder.push(live.item_id)
    state.turnState[TURN_A] = 'running'

    expect(rewindAvailability(state, TURN_A)).toBe('busy')
    // 其他已完成的回合也被线程级运行态挡住：服务端会 409。
    expect(rewindAvailability(state, TURN_B)).toBe('busy')
  })

  it('activeTurnId 优先于 item 状态判定运行态', () => {
    const state = twoFinishedTurns()
    state.activeTurnId = '00000000-0000-0000-0000-0000000000cc'

    expect(hasRunningTurn(state)).toBe(true)
    expect(rewindAvailability(state, TURN_A)).toBe('busy')
  })

  it('只有流式 item、没有 turnState 时仍判定为运行中', () => {
    const state = emptyItemsState()
    const streaming = item(TURN_A, '31', 'streaming')
    state.itemsById[streaming.item_id] = streaming
    state.itemOrder.push(streaming.item_id)

    expect(hasRunningTurn(state)).toBe(true)
    expect(rewindAvailability(state, TURN_A)).toBe('busy')
  })

  it('没有任何可渲染行的回合不显示入口', () => {
    expect(rewindAvailability(emptyItemsState(), TURN_A)).toBe('hidden')
  })

  it('批量映射与逐回合查询结果一致，且一次扫描覆盖所有回合', () => {
    const state = twoFinishedTurns()
    const byTurn = rewindAvailabilityByTurn(state)

    expect([...byTurn.entries()]).toEqual([
      [TURN_A, 'ready'],
      [TURN_B, 'ready']
    ])
    expect(rewindAvailability(state, TURN_B)).toBe(byTurn.get(TURN_B))
    // 没有行的回合不在映射里，画布回退到 hidden。
    expect(byTurn.has('00000000-0000-0000-0000-0000000000cc')).toBe(false)
  })

  it('批量映射把非终态 turnState 的回合标为禁用', () => {
    const state = twoFinishedTurns()
    state.turnState[TURN_A] = 'aborting'

    const byTurn = rewindAvailabilityByTurn(state)

    expect(byTurn.get(TURN_A)).toBe('busy')
    expect(byTurn.get(TURN_B)).toBe('busy')
  })
})

describe('executeRewind', () => {
  it('成功时以 turn_id 调用 API 并回报服务端响应', async () => {
    const response = { last_seq: 41, rewind_from_seq: 12, already_rewound: false }
    const rewind = vi.fn().mockResolvedValue(response)
    const onRewound = vi.fn()
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_A, availability: 'ready' },
      { rewind, onRewound, onError }
    )

    expect(ok).toBe(true)
    expect(rewind).toHaveBeenCalledWith(THREAD_ID, { turn_id: TURN_A })
    expect(onRewound).toHaveBeenCalledWith(response)
    expect(onError).not.toHaveBeenCalled()
  })

  it('线程仍有运行中回合时不发起请求', async () => {
    const rewind = vi.fn()
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_A, availability: 'busy' },
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
      { threadId: THREAD_ID, turnId: TURN_A, availability: 'ready' },
      { rewind, onRewound, onError }
    )

    expect(ok).toBe(false)
    expect(onRewound).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledTimes(1)
    expect((onError.mock.calls[0]?.[0] as ApiError).status).toBe(409)
  })

  it('404 目标不存在走错误通道且不触发重载', async () => {
    const rewind = vi.fn().mockRejectedValue(apiError(404))
    const onError = vi.fn()

    const ok = await executeRewind(
      { threadId: THREAD_ID, turnId: TURN_A, availability: 'ready' },
      { rewind, onRewound: vi.fn(), onError }
    )

    expect(ok).toBe(false)
    expect(onError).toHaveBeenCalledTimes(1)
    expect((onError.mock.calls[0]?.[0] as ApiError).status).toBe(404)
  })
})
