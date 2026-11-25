import { describe, expect, it } from 'vitest'
import { answeredGateCards, approvalCards, groupByTurn, hasLiveItems, splitInheritedPrefix, userInputCards } from './selectors'
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

  it('anchors completed user_input items into their own turns, in stream order', () => {
    const TURN_A = '00000000-0000-0000-0000-00000000000a'
    const TURN_B = '00000000-0000-0000-0000-00000000000b'
    const GATE_A = '00000000-0000-0000-0000-0000000000g1'
    const GATE_B = '00000000-0000-0000-0000-0000000000g2'
    const state = emptyItemsState()
    state.itemOrder = [GATE_A, GATE_B]
    state.itemsById = {
      [GATE_A]: {
        item_id: GATE_A,
        turn_id: TURN_A,
        kind: 'user_input',
        status: 'completed',
        created_at: '2026-10-28T00:00:00Z',
        text: 'pick a branch',
        detail: 'main'
      },
      [GATE_B]: {
        item_id: GATE_B,
        turn_id: TURN_B,
        kind: 'user_input',
        // A gate still waiting for its answer must not render as history.
        status: 'streaming',
        created_at: '2026-10-28T00:00:01Z',
        text: 'pick a tag'
      }
    }

    const cards = answeredGateCards(state)
    expect(cards).toHaveLength(1)
    expect(cards[0]?.turnId).toBe(TURN_A)
    expect(cards[0]?.item.text).toBe('pick a branch')
    expect(cards[0]?.item.detail).toBe('main')
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
    state.liveItemCount = 1
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

  it('只重建发生变化的回合，不枚举未变的 itemOrder', () => {
    const first = baseView()
    const before = groupByTurn(first)
    const guardedOrder = new Proxy(first.itemOrder, {
      get(target, property, receiver) {
        if (property === Symbol.iterator) throw new Error('full stream scan')
        return Reflect.get(target, property, receiver)
      }
    })
    const after = groupByTurn({
      itemOrder: guardedOrder,
      itemsById: { ...first.itemsById, [ITEM_B]: { ...first.itemsById[ITEM_B]!, text: 'ab' } },
      turnState: first.turnState,
      changedItemIds: [ITEM_B]
    }, before, { ...first, itemOrder: guardedOrder })

    expect(after[0]).toBe(before[0])
    expect(after[1]).not.toBe(before[1])
    expect(after[1]?.items[0]?.text).toBe('ab')
  })

  it('does not rescan prior groups to count rows when the ordered channel is unchanged', () => {
    const first = baseView()
    const before = groupByTurn(first)
    const guardedGroups = new Proxy(before, {
      get(target, property, receiver) {
        if (property === 'reduce') throw new Error('prior group count scan')
        return Reflect.get(target, property, receiver)
      }
    })
    const after = groupByTurn({
      itemOrder: first.itemOrder,
      itemsById: { ...first.itemsById, [ITEM_B]: { ...first.itemsById[ITEM_B]!, text: 'ab' } },
      turnState: first.turnState,
      changedItemIds: [ITEM_B]
    }, guardedGroups, first)

    expect(after[0]).toBe(before[0])
    expect(after[1]).not.toBe(before[1])
  })

  it('没有上一个结果时按需重建', () => {
    const groups = groupByTurn(baseView())

    expect(groups.map((group) => group.turnId)).toEqual([TURN_A, TURN_B])
    expect(hasLiveItems(baseView())).toBe(true)
  })

  it('只折叠分叉继承的前缀，保留其余行顺序', () => {
    const view = baseView()
    const { visible, inheritedCount } = splitInheritedPrefix(view, 1)

    expect(inheritedCount).toBe(1)
    expect(visible.itemOrder).toEqual([ITEM_B])
    expect(visible.itemsById).toBe(view.itemsById)
  })

  it('没有继承项时返回原视图，不产生新对象', () => {
    const view = baseView()
    expect(splitInheritedPrefix(view, 0).visible).toBe(view)
  })

  it('继承数超过流长度时只折叠已有行', () => {
    const { visible, inheritedCount } = splitInheritedPrefix(baseView(), 99)

    expect(inheritedCount).toBe(2)
    expect(visible.itemOrder).toEqual([])
  })

  it('uses the reducer live count without scanning the transcript', () => {
    const itemOrder = new Proxy([] as string[], {
      get() { throw new Error('live selector full scan') }
    })
    expect(hasLiveItems({ itemOrder, itemsById: {}, liveItemCount: 1 })).toBe(true)
    expect(hasLiveItems({ itemOrder, itemsById: {}, liveItemCount: 0 })).toBe(false)
  })
})
