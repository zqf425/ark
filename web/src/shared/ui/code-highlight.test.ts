import { describe, expect, it } from 'vitest'
import { highlightSource } from './code-highlight'

describe('highlightSource', () => {
  it('按扩展名选择语言并产出转义后的高亮 HTML', () => {
    const html = highlightSource('const a: number = 1\n', 'src/app.ts')
    expect(html).toContain('hljs-keyword')
    expect(html).not.toContain('<script')
  })

  it('源码中的 HTML 始终被转义', () => {
    const html = highlightSource('const x = "<b>&amp;"\n', 'a.js')
    expect(html).not.toContain('<b>')
    expect(html).toContain('&lt;b&gt;')
  })

  it('未知扩展名回落到自动检测且不抛错', () => {
    const html = highlightSource('plain text\n', 'file.unknownext')
    expect(html.length).toBeGreaterThan(0)
  })

  it('超出上限的源码退化为转义纯文本', () => {
    const big = 'x'.repeat(100_001)
    const html = highlightSource(`<div>${big}`, 'big.html')
    expect(html).not.toContain('<div>')
    expect(html).toContain('&lt;div&gt;')
  })
})
