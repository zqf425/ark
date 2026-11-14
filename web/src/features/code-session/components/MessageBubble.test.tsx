// @vitest-environment jsdom
/**
 * The user bubble renders what the model actually received. An attachment
 * record on the item must become one chip per outcome — with the file name,
 * not an id — and the chip state must track the delivery channel, so a
 * degraded or skipped attachment is visible instead of silently missing.
 * Items without attachments render exactly as before.
 */
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem, TurnAttachmentOutcome } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { UserBubble, AssistantBubble, FinalReplyCard } from './MessageBubble'

installBrowserStubs()

const TURN = '00000000-0000-0000-0000-0000000000aa'

function userItem(overrides: Partial<ThreadItem> = {}): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000u1',
    turn_id: TURN,
    kind: 'user_message',
    status: 'completed',
    created_at: '2026-11-13T12:00:00Z',
    text: '看看这张图',
    ...overrides
  }
}

function outcome(overrides: Partial<TurnAttachmentOutcome>): TurnAttachmentOutcome {
  return {
    attachment_id: '00000000-0000-0000-0000-100000000001',
    original_filename: 'pic.png',
    delivered: 'image',
    ...overrides
  }
}

describe('UserBubble attachments', () => {
  it('无附件的消息不渲染附件区', async () => {
    const harness = await mountWithProviders(<UserBubble item={userItem()} />)
    expect(harness.host.querySelector('.ark-msg__attachments')).toBeNull()
    expect(harness.text()).toContain('看看这张图')
  })

  it('每个投递结果渲染一个 chip，显示文件名与投递通道', async () => {
    const item = userItem({
      attachments: [
        outcome({}),
        outcome({
          attachment_id: '00000000-0000-0000-0000-100000000002',
          original_filename: 'notes.txt',
          delivered: 'preview_text'
        })
      ]
    })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const chips = [...harness.host.querySelectorAll('.ark-msg__attachments .ark-chip')]
    expect(chips).toHaveLength(2)
    const first = chips[0]!
    const second = chips[1]!
    expect(first.textContent).toContain('pic.png')
    expect(first.textContent).toContain('图片')
    expect(second.textContent).toContain('notes.txt')
    expect(second.textContent).toContain('文本预览')
  })

  it('降级/跳过/读取失败的 chip 用警示色并带原因提示', async () => {
    const item = userItem({
      attachments: [
        outcome({ delivered: 'degraded', detail: 'model did not declare vision support' }),
        outcome({
          attachment_id: '00000000-0000-0000-0000-100000000003',
          original_filename: 'big.png',
          delivered: 'skipped',
          detail: 'over the per-turn budget'
        })
      ]
    })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const chips = [...harness.host.querySelectorAll('.ark-msg__attachments .ark-chip')]
    expect(chips).toHaveLength(2)
    for (const chip of chips) {
      expect(chip.className).toContain('ark-chip--warn')
      expect(chip.getAttribute('title')).toBeTruthy()
    }    expect(harness.text()).toContain('未传图（模型无视觉能力）')
    expect(harness.text()).toContain('超出图片预算')
  })

  it('元数据缺失时 chip 回退显示附件 id', async () => {
    const item = userItem({
      attachments: [outcome({ original_filename: undefined, delivered: 'unavailable', detail: 'boom' })]
    })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const chip = harness.host.querySelector('.ark-msg__attachments .ark-chip')
    expect(chip?.textContent).toContain('00000000-0000-0000-0000-100000000001')
    expect(chip?.className).toContain('ark-chip--warn')
  })
})

describe('气泡一键复制', () => {
  afterEach(() => {
    Reflect.deleteProperty(navigator, 'clipboard')
  })

  function stubClipboard(written: string[]): void {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: (text: string) => Promise.resolve(written.push(text)) },
      configurable: true,
      writable: true
    })
  }

  it('用户气泡元信息行带复制按钮,点击复制消息原文', async () => {
    const written: string[] = []
    stubClipboard(written)
    const harness = await mountWithProviders(<UserBubble item={userItem()} />)
    const button = harness.host.querySelector('.ark-msg__copy')
    expect(button).not.toBeNull()
    expect(button?.getAttribute('aria-label')).toBe('复制消息')
    await import('react').then(({ act }) =>
      act(async () => {
        button!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await Promise.resolve()
      })
    )
    expect(written).toEqual(['看看这张图'])
    expect(button?.className).toContain('is-copied')
    await harness.unmount()
  })

  it('助手气泡流式时不渲染复制按钮,完成后可复制原文', async () => {
    const written: string[] = []
    stubClipboard(written)
    const streaming = userItem({
      item_id: '00000000-0000-0000-0000-0000000000a1',
      kind: 'assistant_message',
      status: 'streaming',
      text: '# 标题\n\n正文'
    })
    const done: ThreadItem = { ...streaming, status: 'completed' }
    const live = await mountWithProviders(<AssistantBubble item={streaming} />)
    expect(live.host.querySelector('.ark-msg__copy')).toBeNull()
    await live.unmount()

    const harness = await mountWithProviders(<AssistantBubble item={done} />)
    const button = harness.host.querySelector('.ark-msg__copy')
    expect(button).not.toBeNull()
    await import('react').then(({ act }) =>
      act(async () => {
        button!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await Promise.resolve()
      })
    )
    expect(written).toEqual(['# 标题\n\n正文'])
    await harness.unmount()
  })
})

describe('最终回复卡', () => {
  afterEach(() => {
    Reflect.deleteProperty(navigator, 'clipboard')
  })

  function stubClipboardInCard(written: string[]): void {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: (text: string) => Promise.resolve(written.push(text)) },
      configurable: true,
      writable: true
    })
  }

  const replyItem = (status: ThreadItem['status'], text = '结论如下'): ThreadItem => ({
    item_id: '00000000-0000-0000-0000-0000000000f1',
    turn_id: TURN,
    kind: 'assistant_message',
    status,
    created_at: '2026-11-13T12:00:00Z',
    text
  })

  it('带"最终回复"标题行与复制按钮', async () => {
    const written: string[] = []
    stubClipboardInCard(written)
    const harness = await mountWithProviders(<FinalReplyCard item={replyItem('completed')} />)
    expect(harness.text()).toContain('最终回复')
    const button = harness.host.querySelector('.ark-reply__copy')
    expect(button?.getAttribute('aria-label')).toBe('复制消息')
    // 复制动作位于卡底 footer 行，不在标题行右上角。
    expect(button?.closest('.ark-reply__foot')).not.toBeNull()
    expect(harness.host.querySelector('.ark-reply__head .ark-reply__copy')).toBeNull()
    await import('react').then(({ act }) =>
      act(async () => {
        button!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await Promise.resolve()
      })
    )
    expect(written).toEqual(['结论如下'])
    await harness.unmount()
  })

  it('流式时不渲染复制按钮,正文仍跟随流式文本', async () => {
    const harness = await mountWithProviders(<FinalReplyCard item={replyItem('streaming')} />)
    expect(harness.host.querySelector('.ark-reply__copy')).toBeNull()
    expect(harness.host.querySelector('.ark-caret')).not.toBeNull()
    expect(harness.text()).toContain('结论如下')
    await harness.unmount()
  })

  it('失败状态以错误色标记并展示失败 chip', async () => {
    const harness = await mountWithProviders(<FinalReplyCard item={replyItem('failed')} />)
    expect(harness.host.querySelector('.ark-reply.is-failed')).not.toBeNull()
    expect(harness.host.querySelector('.ark-chip--error')).not.toBeNull()
    await harness.unmount()
  })
})
