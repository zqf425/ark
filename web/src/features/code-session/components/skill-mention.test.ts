// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { splitSkillMentions } from './skill-mention'
import type { SkillSummary } from '../../../shared/contracts'

function skill(id: string): SkillSummary {
  return {
    id,
    name: id,
    description: undefined,
    version: '0.0.0',
    root: `/skills/${id}`,
    entry_path: `/skills/${id}/SKILL.md`,
    priority: 0,
    source: 'base',
    legacy: false
  }
}

const SKILLS = [skill('kun-new-session'), skill('pdf'), skill('go-deploy')]

describe('splitSkillMentions', () => {
  it('识别消息开头的技能提及', () => {
    const segments = splitSkillMentions('/kun-new-session 帮我新开会话', SKILLS)
    expect(segments).toEqual([
      { kind: 'mention', value: '/kun-new-session', skill: SKILLS[0] },
      { kind: 'text', value: ' 帮我新开会话' }
    ])
  })

  it('识别句中提及且大小写不敏感', () => {
    const segments = splitSkillMentions('先跑 /PDF 再总结', SKILLS)
    expect(segments).toHaveLength(3)
    expect(segments[1]).toMatchObject({ kind: 'mention', value: '/PDF' })
  })

  it('未知斜杠词保持纯文本', () => {
    const segments = splitSkillMentions('看看 /usr/local 与 /pdf-tools-plus', SKILLS)
    expect(segments).toEqual([{ kind: 'text', value: '看看 /usr/local 与 /pdf-tools-plus' }])
  })

  it('更长 id 优先，避免前缀误配', () => {
    const segments = splitSkillMentions('/kun-new-session', [skill('kun-new'), SKILLS[0] as SkillSummary])
    expect(segments).toEqual([{ kind: 'mention', value: '/kun-new-session', skill: SKILLS[0] }])
  })

  it('无技能列表时整体为纯文本', () => {
    expect(splitSkillMentions('/pdf 提取', [])).toEqual([{ kind: 'text', value: '/pdf 提取' }])
  })

  it('空文本返回空段', () => {
    expect(splitSkillMentions('', SKILLS)).toEqual([])
  })
})
