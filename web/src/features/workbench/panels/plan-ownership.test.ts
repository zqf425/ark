import { describe, expect, it } from 'vitest'
import { adoptServerGoal, beginPlanFetch, isLatestPlanRequest, isPlanOwner, resetPlanOwnership, shouldApplyPlanResponse } from './plan-ownership'

describe('resetPlanOwnership', () => {
  it('切换 threadId 后归属改为新线程，并清空服务器 goal 基线', () => {
    const before = { owner: 'thread-a', serverGoal: '把 A 的活干完', generation: 4 }
    const after = resetPlanOwnership('thread-b', before.generation)
    expect(after.owner).toBe('thread-b')
    expect(after.serverGoal).toBe('')
  })

  it('切换 threadId 会推进代次，使 A 线程晚到的 getPlan 响应失效', () => {
    const before = { owner: 'thread-a', serverGoal: 'goal-a', generation: 4 }
    const after = resetPlanOwnership('thread-b', before.generation)
    expect(after.generation).toBe(5)
    expect(isLatestPlanRequest(after.generation, before.generation)).toBe(false)
  })

  it('取消选择会话时归属为空，任何写回都不再被允许', () => {
    const after = resetPlanOwnership(null, 1)
    expect(after.owner).toBeNull()
    expect(isPlanOwner(after.owner, 'thread-b')).toBe(false)
  })
})

describe('isPlanOwner', () => {
  it('归属与当前线程一致时才允许写回', () => {
    expect(isPlanOwner('thread-b', 'thread-b')).toBe(true)
  })

  it('残留的 A 线程 plan 不能写进 B 线程', () => {
    expect(isPlanOwner('thread-a', 'thread-b')).toBe(false)
  })

  it('尚未加载任何 plan 时不允许写回', () => {
    expect(isPlanOwner(null, 'thread-b')).toBe(false)
  })
})

describe('adoptServerGoal', () => {
  it('未编辑的草稿跟随服务器最新值', () => {
    expect(adoptServerGoal('old goal', 'old goal', 'new goal')).toBe('new goal')
  })

  it('用户已编辑、尚未保存的草稿不会被事件驱动的重拉覆盖', () => {
    expect(adoptServerGoal('我正在写的目标', 'old goal', 'new goal')).toBe('我正在写的目标')
  })

  it('用户清空草稿也视为编辑，不会被服务器值填回', () => {
    expect(adoptServerGoal('', 'old goal', 'new goal')).toBe('')
  })
})

describe('isLatestPlanRequest', () => {
  it('被保存动作或新请求超越的响应不会覆盖计划', () => {
    expect(isLatestPlanRequest(7, 6)).toBe(false)
    expect(isLatestPlanRequest(7, 7)).toBe(true)
  })
})

describe('beginPlanFetch', () => {
  it('首次挂载先把归属交给线程，避免第一个 getPlan 因无归属被丢弃', () => {
    const started = beginPlanFetch({ owner: null, serverGoal: '', generation: 0 }, 'thread-a')
    expect(started.owner).toBe('thread-a')
    expect(shouldApplyPlanResponse(started, 'thread-a', started.generation)).toBe(true)
  })

  it('同线程重拉保留服务器 goal 基线，跨线程重拉丢弃', () => {
    const same = beginPlanFetch({ owner: 'thread-a', serverGoal: 'goal-a', generation: 2 }, 'thread-a')
    expect(same.serverGoal).toBe('goal-a')
    const switched = beginPlanFetch({ owner: 'thread-a', serverGoal: 'goal-a', generation: 2 }, 'thread-b')
    expect(switched.serverGoal).toBe('')
  })
})

describe('shouldApplyPlanResponse', () => {
  it('线程归属已切走时，A 线程晚到的响应不落到 B 上', () => {
    const ownership = beginPlanFetch({ owner: 'thread-a', serverGoal: '', generation: 3 }, 'thread-b')
    expect(shouldApplyPlanResponse(ownership, 'thread-a', 3)).toBe(false)
  })

  it('保存推进代次后，更早发起的 getPlan 响应不得覆盖刚保存的结果', () => {
    const ownership = beginPlanFetch({ owner: 'thread-a', serverGoal: '', generation: 3 }, 'thread-a')
    const fetchGeneration = ownership.generation
    const afterSave = { ...ownership, generation: ownership.generation + 1 }
    expect(shouldApplyPlanResponse(afterSave, 'thread-a', fetchGeneration)).toBe(false)
  })
})
