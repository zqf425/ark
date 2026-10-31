import { beforeEach, describe, expect, it } from 'vitest'
import { useCodeSessionStore } from './store'

describe('stream reload token', () => {
  beforeEach(() => {
    useCodeSessionStore.getState().resetItems()
  })

  it('requestStreamReload 递增 token 以驱动 detail 重拉与 SSE 重订阅', () => {
    const before = useCodeSessionStore.getState().reloadToken
    useCodeSessionStore.getState().requestStreamReload()
    expect(useCodeSessionStore.getState().reloadToken).toBe(before + 1)
  })

  it('resetItems 不重置 reloadToken（重载期间 token 保持单调）', () => {
    useCodeSessionStore.getState().requestStreamReload()
    const token = useCodeSessionStore.getState().reloadToken
    useCodeSessionStore.getState().resetItems()
    expect(useCodeSessionStore.getState().reloadToken).toBe(token)
  })
})
