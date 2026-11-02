import { describe, expect, it } from 'vitest'
import { memoryTextByteLength, memoryTextError, parseMemoryTags } from './memory-form'

describe('memoryTextError', () => {
  it('空文本必填，超 16KiB UTF-8 字节拒绝', () => {
    expect(memoryTextError('   ')).toBe('settings.memory.textRequired')
    expect(memoryTextError('ok')).toBeNull()
    // 3 bytes per CJK char: 6000 chars ≈ 18k bytes.
    expect(memoryTextError('中'.repeat(6000))).toBe('settings.memory.textTooLong')
    expect(memoryTextError('中'.repeat(5000))).toBeNull()
  })

  it('字节数按 UTF-8 而不是字符数', () => {
    expect(memoryTextByteLength('a')).toBe(1)
    expect(memoryTextByteLength('中')).toBe(3)
  })
})

describe('parseMemoryTags', () => {
  it('中英文逗号分隔、去空白、去重', () => {
    expect(parseMemoryTags(' a ，b,,a, c ')).toEqual(['a', 'b', 'c'])
    expect(parseMemoryTags('')).toEqual([])
  })
})
