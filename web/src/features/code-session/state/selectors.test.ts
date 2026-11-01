import { describe, expect, it } from 'vitest'
import { approvalCards, groupByTurn, hasLiveItems, userInputCards } from './selectors'
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

/**
 * `groupByTurn` is the projection behind the memoized turn blocks. Reusing a
 * group object whose rows did not change is what keeps earlier turns from
 * re-rendering while a token streams into the newest one.
 */
describe('groupByTurn reference reuse', () => {
  const TURN_A = '00000000-0000-0000-0000-00000000000a'
  const TURN_B = '00000000-0000-0000-0000-00000000000b'
  const ITEM_A = '00000000-0000-0000-0000-00000000000c'
  const ITEM_B = '00000000-0000-0000-0000-00000000000d'

  function baseView(): ReturnType<typeof emptyItemsState> {
    const state = emptyItemsState()
    state.itemOrder = [ITEM_A, ITEM_B]
    state.itemsById = {
      [ITEM_A]: {
        item_id: ITEM_A,
        turn_id: TURN_A,
        kind: 'assistant_message',
        status: 'completed',
        created_at: '2026-10-28T00:00:00Z',
        text: 'done'
      },
      [ITEM_B]: {
        item_id: ITEM_B,
        turn_id: TURN_B,
        kind: 'assistant_message',
        status: 'streaming',
        created_at: '2026-10-28T00:00:01Z',
        text: 'a'
      }
    }
    return state
  }

  it('未变化的回合保留上一个分组对象', () => {
    const first = baseView()
    const before = groupByTurn(first)

    // Only turn B streams: a new item object and a new itemsById map.
    const after = groupByTurn(
      { itemOrder: first.itemOrder, itemsById: { ...first.itemsById, [ITEM_B]: { ...first.itemsById[ITEM_B]!, text: 'ab' } }, turnState: first.turnState },
      before
    )

    expect(after).toHaveLength(2)
    expect(after[0]).toBe(before[0])
    expect(after[1]).not.toBe(before[1])
    expect(after[1]?.items[0]?.text).toBe('ab')
  })

  it('回合状态变化会让该回合分组失效', () => {
    const first = baseView()
    const before = groupByTurn(first)
    const after = groupByTurn({ ...first, turnState: { [TURN_A]: 'completed' } }, before)

    expect(after[0]).not.toBe(before[0])
    expect(after[0]?.state).toBe('completed')
    expect(after[1]).toBe(before[1])
  })

  it('行数变化会让该回合分组失效，且顺序保持服务端顺序', () => {
    const first = baseView()
    const before = groupByTurn(first)
    const extra = { ...first.itemsById[ITEM_B]!, item_id: '00000000-0000-0000-0000-00000000000e' }
    const after = groupByTurn(
      { itemOrder: [...first.itemOrder, extra.item_id], itemsById: { ...first.itemsById, [extra.item_id]: extra }, turnState: first.turnState },
      before
    )

    expect(after[0]).toBe(before[0])
    expect(after[1]?.items).toHaveLength(2)
    expect(after.map((group) => group.turnId)).toEqual([TURN_A, TURN_B])
  })

  it('没有上一个结果时按需重建', () => {
    const groups = groupByTurn(baseView())

    expect(groups.map((group) => group.turnId)).toEqual([TURN_A, TURN_B])
    expect(hasLiveItems(baseView())).toBe(true)
  })
})
