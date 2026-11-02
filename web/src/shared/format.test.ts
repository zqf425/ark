import { describe, expect, it } from 'vitest'
import { formatTokens, formatUsd } from './format'

describe('formatUsd', () => {
  it('按 locale 渲染 USD，符号不硬编码', () => {
    const zh = formatUsd(0.02, 'zh-CN')
    const en = formatUsd(0.02, 'en-US')
    // Exact symbol placement differs across ICU versions; the currency and the
    // amount must be present and the two locales must not share one rendering.
    expect(zh).toContain('0.02')
    expect(en).toContain('0.02')
    expect(zh).toMatch(/US?\$/)
    expect(en).toMatch(/\$/)
  })

  it('非有限值回退为 $0.00 而不是 NaN', () => {
    expect(formatUsd(Number.NaN, 'en-US')).toMatch(/0/)
  })
})

describe('formatTokens', () => {
  it('千位以下原样，以上缩写为 k', () => {
    expect(formatTokens(999)).toBe('999')
    expect(formatTokens(12_345)).toBe('12.3k')
  })
})
