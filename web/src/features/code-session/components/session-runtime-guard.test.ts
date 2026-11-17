import { describe, expect, it } from 'vitest'
import { canSaveRuntime } from './session-runtime-guard'

describe('canSaveRuntime', () => {
  it('仅当表单数据属于当前线程时才允许保存', () => {
    expect(canSaveRuntime('thread-b', 'thread-b')).toBe(true)
  })

  it('A 线程加载的配置不能写进 B 线程', () => {
    expect(canSaveRuntime('thread-a', 'thread-b')).toBe(false)
  })

  it('线程加载失败或尚未加载时禁止保存，避免用空覆盖写掉他人配置', () => {
    expect(canSaveRuntime(null, 'thread-b')).toBe(false)
  })
})
