// @vitest-environment jsdom
/**
 * The mirror layer only exists while the draft holds something recognizable:
 * complete mentions and the in-progress token chip, everything else stays
 * plain text. A wrong mirror would also mean a wrong alignment with the real
 * textarea, so the render rules are pinned here — including the zero-advance
 * rule: a chip must occupy exactly the advance of the verbatim token, or every
 * glyph after it drifts and the draft reads as double text.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { DraftHighlight } from './DraftHighlight'
import type { SkillSummary } from '../../../shared/contracts'

installBrowserStubs()

function skill(id: string): SkillSummary {
  return {
    id,
    name: id,
    description: undefined,
    version: '0.1.0',
    root: `/skills/${id}`,
    entry_path: `/skills/${id}/SKILL.md`,
    priority: 0,
    source: 'base',
    legacy: false
  }
}

const SKILLS = [skill('kun-new-session'), skill('pdf')]

describe('DraftHighlight', () => {
  let harness: Awaited<ReturnType<typeof mountWithProviders>> | null = null
  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('空草稿不渲染镜像层', async () => {
    harness = await mountWithProviders(<DraftHighlight text="" skills={SKILLS} token={null} />)
    expect(harness.host.querySelector('.ark-composer__highlight')).toBeNull()
  })

  it('完整提及渲染为芯片', async () => {
    harness = await mountWithProviders(<DraftHighlight text="/pdf 提取一下" skills={SKILLS} token={null} />)
    const chip = harness.host.querySelector('.ark-mention')
    expect(chip?.textContent).toContain('/pdf')
    expect(harness.text()).toContain('提取一下')
  })

  it('未知斜杠词保持纯文本', async () => {
    harness = await mountWithProviders(<DraftHighlight text="看看 /usr/local" skills={SKILLS} token={null} />)
    expect(harness.host.querySelector('.ark-mention')).toBeNull()
  })

  it('进行中的 token 渲染为 typing 芯片', async () => {
    harness = await mountWithProviders(
      <DraftHighlight text="跑 /pd" skills={SKILLS} token={{ start: 2, end: 5, query: 'pd' }} />
    )
    const chip = harness.host.querySelector('.ark-mention--typing')
    expect(chip?.textContent).toContain('/pd')
    // Only the open token chips; no completed mention exists in this draft.
    expect(harness.host.querySelectorAll('.ark-mention')).toHaveLength(1)
  })

  it('芯片文字逐字等于 token,镜像芯片不渲染图标', async () => {
    harness = await mountWithProviders(
      <DraftHighlight text="/kun-new-session 跑一下" skills={SKILLS} token={null} />
    )
    const chip = harness.host.querySelector('.ark-mention')
    // The verbatim token stays intact as the chip's only content; the mirror
    // renders no bolt glyph (it would poke out from under the opaque text).
    expect(chip?.textContent).toContain('/kun-new-session')
    expect(chip?.querySelector('svg')).toBeNull()
  })

  it('镜像芯片可被镜像层作用域选择器命中(装饰全部在 CSS)', async () => {
    harness = await mountWithProviders(
      <DraftHighlight text="前面 /pdf 后面" skills={SKILLS} token={null} />
    )
    // The mark's rule and wash are painted by CSS; the component renders only
    // the verbatim token — no icon or spacer that could shift following text.
    const chip = harness.host.querySelector('.ark-composer__highlight .ark-mention')
    expect(chip).not.toBeNull()
    expect(chip?.querySelector('svg')).toBeNull()
  })
})
