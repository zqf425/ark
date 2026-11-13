import { describe, expect, it } from 'vitest'
import type { ThreadItem, TurnId } from '../../../shared/contracts'
import { emptyItemsState } from './items-reducer'
import { formatElapsed, liveActivity, reasoningTrail, type ActivityView } from './live-activity'

const TURN = '00000000-0000-0000-0000-0000000000t1' as TurnId
const OTHER = '00000000-0000-0000-0000-0000000000t9' as TurnId

function item(partial: Partial<ThreadItem> & { item_id: string }): ThreadItem {
  return {
    turn_id: TURN,
    kind: 'reasoning',
    status: 'streaming',
    created_at: '2026-11-13T14:00:00Z',
    ...partial
  } as ThreadItem
}

function view(items: ThreadItem[], overrides?: Partial<ActivityView>): ActivityView {
  const base = emptyItemsState()
  for (const entry of items) base.itemsById[entry.item_id] = entry
  base.itemOrder = items.map((entry) => entry.item_id)
  return { ...base, ...overrides }
}

function runningTurn(): Partial<ActivityView> {
  return { activeTurnId: TURN, turnState: { [TURN]: 'running' } }
}

describe('liveActivity', () => {
  it('没有活跃回合时不显示活动行', () => {
    expect(liveActivity(view([]))).toBeNull()
  })

  it('回合结束后不再显示活动行', () => {
    expect(
      liveActivity(view([item({ item_id: 'i1' })], { activeTurnId: TURN, turnState: { [TURN]: 'completed' } }))
    ).toBeNull()
  })

  it('活跃回合中返回最新的流式 item', () => {
    const older = item({ item_id: 'a', status: 'completed' })
    const newer = item({ item_id: 'b', kind: 'assistant_message', text: 'hi' })
    const projection = liveActivity(view([older, newer], runningTurn()))
    expect(projection?.item.item_id).toBe('b')
  })

  it('纯思考阶段返回 reasoning 流式 item', () => {
    const projection = liveActivity(view([item({ item_id: 'r1', text: 'thinking…' })], runningTurn()))
    expect(projection?.item.kind).toBe('reasoning')
  })

  it('其他回合的流式 item 不算数', () => {
    const foreign = item({ item_id: 'x', turn_id: OTHER })
    expect(liveActivity(view([foreign], runningTurn()))).toBeNull()
  })
})

describe('reasoningTrail', () => {
  it('短文本原样返回并去首尾空白', () => {
    expect(reasoningTrail('  thinking  ')).toBe('thinking')
  })

  it('长文本只保留尾部', () => {
    expect(reasoningTrail('x'.repeat(200))).toBe('x'.repeat(140))
  })
})

describe('formatElapsed', () => {
  it('按秒累计并格式化为 m:ss', () => {
    expect(formatElapsed('2026-11-13T14:00:00Z', Date.parse('2026-11-13T14:01:05Z'))).toBe('1:05')
  })

  it('无效时间戳返回 null', () => {
    expect(formatElapsed('not-a-time', 0)).toBeNull()
  })
})
