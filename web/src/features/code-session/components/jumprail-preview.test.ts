import { describe, expect, it } from 'vitest'
import { JUMPRAIL_PREVIEW_LIMIT, turnLabelPreview } from './jumprail-preview'

describe('turnLabelPreview', () => {
  it('原样返回不超过上限的文本', () => {
    expect(turnLabelPreview('修复登录页 bug')).toBe('修复登录页 bug')
  })

  it('超过 15 字符时只保留前 15 字符并以 ... 结尾', () => {
    const text = '帮我梳理这个模块的完整设计思路并给出落地步骤清单'
    expect(turnLabelPreview(text)).toBe(`${text.slice(0, 15)}...`)
  })

  it('常量为 15，防止无意收紧或放宽', () => {
    expect(JUMPRAIL_PREVIEW_LIMIT).toBe(15)
  })

  it('按码点截断，不劈开代理对（emoji）', () => {
    const text = '🚀'.repeat(20)
    expect(turnLabelPreview(text)).toBe(`${'🚀'.repeat(15)}...`)
  })

  it('去掉首尾空白后再计算', () => {
    expect(turnLabelPreview('   修复登录页  ')).toBe('修复登录页')
  })

  it('多行输入折叠为一行，避免换行进 tooltip', () => {
    expect(turnLabelPreview('第一行\n第二行')).toBe('第一行 第二行')
    expect(turnLabelPreview('第一行\n\n\n第二行')).toBe('第一行 第二行')
  })

  it('空文本与空白文本返回空串，由调用方回落到回合序号', () => {
    expect(turnLabelPreview(null)).toBe('')
    expect(turnLabelPreview(undefined)).toBe('')
    expect(turnLabelPreview('')).toBe('')
    expect(turnLabelPreview('   \n  ')).toBe('')
  })
})
