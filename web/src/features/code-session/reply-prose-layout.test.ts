/**
 * 回复区排版钉子：两件事不能回退——
 * 1. Markdown 宿主（助手气泡正文 / 最终回复卡正文）不得是 pre-wrap：marked
 *    在块级元素之间输出换行文本节点，pre-wrap 会把每个换行画成真实空行，
 *    叠加块 margin 后变成"每段之间一行空行"的巨大间隔；
 * 2. 最终回复卡的 footer 行带生成时间（与用户气泡 footer 的时钟同款）。
 * CSS 无法在 jsdom 里计算，故按 general-section-i18n.test 的先例读源码钉住。
 */
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const BUBBLES_CSS = 'features/code-session/code-session-bubbles.css'
const MARKDOWN_CSS = 'shared/ui/markdown.css'
const MESSAGE_BUBBLE = 'features/code-session/components/MessageBubble.tsx'

function readSource(relative: string): string {
  return readFileSync(fileURLToPath(new URL(`../../${relative}`, import.meta.url)), 'utf8')
}

/**
 * Pull one top-level `selector { … }` block. The selector must start at a
 * line beginning: `indexOf` alone would also hit the tail of a longer
 * compound selector (`.ark-msg--assistant .ark-msg__body--prose {`).
 */
function cssBlock(css: string, selector: string): string {
  const start = css.indexOf(`\n${selector} {`)
  if (start < 0) return ''
  const end = css.indexOf('\n}', start)
  return css.slice(start, end)
}

describe('Markdown 宿主不得 pre-wrap（块间换行会被画成空行）', () => {
  it('助手气泡与最终回复卡的正文宿主是 white-space: normal', () => {
    const bubbles = readSource(BUBBLES_CSS)
    expect(cssBlock(bubbles, '.ark-msg__body--prose')).toContain('white-space: normal')
    expect(cssBlock(bubbles, '.ark-reply__body')).toContain('white-space: normal')
    // 纯文本变体（用户气泡等）保持 pre-wrap 不受影响。
    expect(cssBlock(bubbles, '.ark-msg__body')).toContain('white-space: pre-wrap')
  })

  it('共享 .ark-md 宿主自身声明 normal，防止其他挂载点复现', () => {
    const markdown = readSource(MARKDOWN_CSS)
    expect(cssBlock(markdown, '.ark-md')).toContain('white-space: normal')
  })
})

describe('最终回复卡 footer 带生成时间', () => {
  it('footer 行渲染 ark-reply__time 且使用 item.created_at', () => {
    const bubble = readSource(MESSAGE_BUBBLE)
    const footStart = bubble.indexOf('ark-reply__foot')
    expect(footStart).toBeGreaterThan(0)
    const footBody = bubble.slice(footStart, bubble.indexOf('</footer>', footStart))
    expect(footBody).toContain('ark-reply__time')
    expect(footBody).toContain('formatClock(item.created_at, locale)')
  })
})
