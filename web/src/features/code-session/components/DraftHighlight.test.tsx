// @vitest-environment jsdom
/**
 * The mirror layer only exists while the draft holds something recognizable:
 * complete mentions and the in-progress token chip, everything else stays
 * plain text. A wrong mirror would also mean a wrong alignment with the real
 * textarea, so the render rules are pinned here.
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
})
