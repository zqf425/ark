// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest'
import { loadPersistedDrafts, persistDrafts } from './draft-storage'
import { useCodeSessionStore } from './store'

const STORAGE_KEY = 'ark.composer.drafts'
const A = '00000000-0000-0000-0000-00000000000a'
const B = '00000000-0000-0000-0000-00000000000b'

function memoryStorage(): Storage {
  const entries = new Map<string, string>()
  return {
    get length() {
      return entries.size
    },
    clear: () => entries.clear(),
    getItem: (key) => entries.get(key) ?? null,
    key: (index) => [...entries.keys()][index] ?? null,
    removeItem: (key) => entries.delete(key),
    setItem: (key, value) => entries.set(key, value)
  }
}

describe('draft-storage', () => {
  beforeEach(() => {
    Object.defineProperty(window, 'localStorage', { value: memoryStorage(), configurable: true })
  })

  it('persistDrafts 写入、loadPersistedDrafts 读回同一映射', () => {
    persistDrafts({ [A]: 'A 的草稿', [B]: 'B 的草稿' })
    expect(loadPersistedDrafts()).toEqual({ [A]: 'A 的草稿', [B]: 'B 的草稿' })
  })

  it('损坏或类型不符的载荷读为空，不抛错', () => {
    window.localStorage.setItem(STORAGE_KEY, '{broken json')
    expect(loadPersistedDrafts()).toEqual({})
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify({ [A]: 42 }))
    expect(loadPersistedDrafts()).toEqual({})
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(['a', 'b']))
    expect(loadPersistedDrafts()).toEqual({})
  })

  it('超长草稿被钳制到上限字符数', () => {
    persistDrafts({ [A]: 'x'.repeat(150_000) })
    const loaded = loadPersistedDrafts()
    expect(loaded[A]).toHaveLength(100_000)
  })

  it('超出条目上限时保留最新写入的条目', () => {
    const many: Record<string, string> = {}
    for (let i = 0; i < 130; i += 1) {
      const id = `00000000-0000-0000-0000-${String(i).padStart(12, '0')}`
      many[id] = `草稿 ${i}`
    }
    persistDrafts(many)
    const loaded = loadPersistedDrafts()
    expect(Object.keys(loaded)).toHaveLength(100)
    // The oldest keys (head of insertion order) are the dropped ones.
    expect(loaded['00000000-0000-0000-0000-000000000000']).toBeUndefined()
    expect(loaded['00000000-0000-0000-0000-000000000129']).toBe('草稿 129')
  })
})

describe('runtime slice draft persistence', () => {
  beforeEach(() => {
    Object.defineProperty(window, 'localStorage', { value: memoryStorage(), configurable: true })
    useCodeSessionStore.setState({ drafts: {} })
  })

  it('setDraft / clearDraft 同步镜像到 localStorage', () => {
    useCodeSessionStore.getState().setDraft(A, '未发送的内容')
    expect(JSON.parse(window.localStorage.getItem(STORAGE_KEY)!)).toEqual({ [A]: '未发送的内容' })

    useCodeSessionStore.getState().clearDraft(A)
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('{}')
  })

  it('hydrateDrafts 从 localStorage 恢复各线程草稿', () => {
    persistDrafts({ [A]: 'A 的草稿', [B]: '' })
    useCodeSessionStore.getState().hydrateDrafts()
    expect(useCodeSessionStore.getState().drafts[A]).toBe('A 的草稿')
    // An empty stored text stays an empty string: a cleared draft must not
    // resurrect its old content on the next hydration.
    expect(useCodeSessionStore.getState().drafts[B]).toBe('')
  })
})
