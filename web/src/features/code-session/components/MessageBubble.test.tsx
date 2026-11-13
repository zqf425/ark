// @vitest-environment jsdom
/**
 * The user bubble renders what the model actually received. An attachment
 * record on the item must become one chip per outcome — with the file name,
 * not an id — and the chip state must track the delivery channel, so a
 * degraded or skipped attachment is visible instead of silently missing.
 * Items without attachments render exactly as before.
 */
import { describe, expect, it } from 'vitest'
import type { ThreadItem, TurnAttachmentOutcome } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { UserBubble } from './MessageBubble'

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
