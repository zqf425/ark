// @vitest-environment jsdom
/**
 * The user bubble renders what the model actually received. An attachment
 * record on the item must become one preview per outcome — image outcomes as
 * clickable thumbnails (the shared lightbox), everything else as a filename
 * pill with the delivery channel in its tooltip. Items without attachments
 * render exactly as before, and the bubble carries no "you" identity row.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ThreadItem, TurnAttachmentOutcome } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { UserBubble, AssistantBubble, FinalReplyCard } from './MessageBubble'
import * as attachmentsApi from '../api/attachments-api'

installBrowserStubs()

// jsdom cannot decode image bytes; every preview fetch resolves to a fake
// object URL. Reaching the lightbox only needs the URL to exist.
vi.spyOn(attachmentsApi, 'fetchAttachmentObjectUrl').mockResolvedValue('blob:mock')

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

  it('每个投递结果渲染一个缩略图，图片可点击打开灯箱', async () => {
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
    const thumbs = [...harness.host.querySelectorAll('.ark-msg__attachments .ark-thumb')]
    expect(thumbs).toHaveLength(2)
    const first = thumbs[0]!
    const second = thumbs[1]!
    // The image preview is a button (click → lightbox) with the file name.
    expect(first.className).toContain('ark-thumb--image')
    expect(first.getAttribute('title')).toBe('pic.png')
    expect(first.getAttribute('aria-haspopup')).toBe('dialog')
    expect(first.tagName).toBe('BUTTON')
    // Non-image previews stay quiet filename pills.
    expect(second.className).toContain('ark-thumb--file')
    expect(second.textContent).toContain('notes.txt')
    expect(second.tagName).not.toBe('BUTTON')
  })

  it('点击图片缩略图打开灯箱，Esc/关闭按钮可退出', async () => {
    const item = userItem({ attachments: [outcome({})] })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const thumb = harness.host.querySelector<HTMLButtonElement>('.ark-thumb--image')
    expect(thumb).not.toBeNull()
    await import('react').then(({ act }) =>
      act(async () => {
        thumb!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await Promise.resolve()
      })
    )
    const dialog = harness.host.querySelector('.ark-lightbox')
    expect(dialog).not.toBeNull()
    expect(dialog?.getAttribute('aria-label')).toBe('pic.png')
    // The close button exits.
    const close = harness.host.querySelector<HTMLButtonElement>('.ark-lightbox__close')
    expect(close).not.toBeNull()
    await import('react').then(({ act }) =>
      act(async () => {
        close!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await Promise.resolve()
      })
    )
    expect(harness.host.querySelector('.ark-lightbox')).toBeNull()
    await harness.unmount()
  })

  it('降级/跳过的附件带警示环与投递通道提示', async () => {
    const item = userItem({
      attachments: [
        outcome({ delivered: 'degraded', detail: 'model did not declare vision support' }),
        outcome({
          attachment_id: '00000000-0000-0000-0000-100000000003',
          original_filename: 'notes-degraded.txt',
          delivered: 'skipped',
          detail: 'over the per-turn budget'
        })
      ]
    })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const wrappers = [...harness.host.querySelectorAll('.ark-msg__attachments .ark-msg-attachment')]
    expect(wrappers).toHaveLength(2)
    for (const wrapper of wrappers) {
      expect(wrapper.className).toContain('ark-msg-attachment--warn')
      // The delivery channel lives in the wrapper's title, not inline text.
      expect(wrapper.getAttribute('title')).toBeTruthy()
    }
    // An image-shaped outcome renders as a thumbnail; the file name lives in
    // its alt/title (jsdom cannot decode bytes, so no real pixels).
    const thumb = wrappers[1]!.querySelector('img')
    expect(thumb?.getAttribute('alt')).toBe('notes-degraded.txt')
  })

  it('元数据缺失时回退显示附件 id', async () => {
    const item = userItem({
      attachments: [outcome({ original_filename: undefined, delivered: 'unavailable', detail: 'boom' })]
    })
    const harness = await mountWithProviders(<UserBubble item={item} />)
    const pill = harness.host.querySelector('.ark-msg__attachments .ark-thumb--file')
    expect(pill?.textContent).toContain('00000000-0000-0000-0000-100000000001')
    expect(pill?.className).toContain('ark-thumb--warn')
  })

  it('用户气泡不再渲染「你」身份行，只保留时间与复制', async () => {
    const harness = await mountWithProviders(<UserBubble item={userItem()} />)
    expect(harness.text()).not.toContain('你')
    expect(harness.host.querySelector('.ark-msg__avatar--you')).toBeNull()
    expect(harness.host.querySelector('.ark-msg__author')).toBeNull()
    expect(harness.host.querySelector('.ark-msg__time')).not.toBeNull()
    expect(harness.host.querySelector('.ark-msg__copy')).not.toBeNull()
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

  it('完成后 footer 行带生成时间,与用户气泡 footer 同款时钟', async () => {
    const harness = await mountWithProviders(<FinalReplyCard item={replyItem('completed')} />)
    const time = harness.host.querySelector('.ark-reply__foot time')
    expect(time).not.toBeNull()
    expect(time?.textContent).not.toBe('')
    // 时钟语义与用户气泡一致：local HH:MM:SS。
    expect(harness.text()).toMatch(/\d{2}:\d{2}:\d{2}/)
    await harness.unmount()
  })

  it('失败状态以错误色标记并展示失败 chip', async () => {
    const harness = await mountWithProviders(<FinalReplyCard item={replyItem('failed')} />)
    expect(harness.host.querySelector('.ark-reply.is-failed')).not.toBeNull()
    expect(harness.host.querySelector('.ark-chip--error')).not.toBeNull()
    await harness.unmount()
  })
})
