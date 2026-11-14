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

import { filterSkills, mentionTokenAt } from './skill-mention'

describe('mentionTokenAt', () => {
  it('识别光标下的未完成提及', () => {
    expect(mentionTokenAt('/kun-new', 8)).toEqual({ start: 0, end: 8, query: 'kun-new' })
    expect(mentionTokenAt('前面 /pd 采', 6)).toEqual({ start: 3, end: 6, query: 'pd' })
  })

  it('裸斜杠 query 为空，可带出全量列表', () => {
    expect(mentionTokenAt('/', 1)).toEqual({ start: 0, end: 1, query: '' })
    expect(mentionTokenAt('run /', 5)).toEqual({ start: 4, end: 5, query: '' })
  })

  it('路径与 URL 不唤起：斜杠前是词字符', () => {
    expect(mentionTokenAt('foo/bar', 7)).toBeNull()
    expect(mentionTokenAt('https://x', 9)).toBeNull()
  })

  it('token 中出现空白后不再唤起（a / b）', () => {
    expect(mentionTokenAt('a / b', 4)).toBeNull()
  })

  it('光标在 token 外为 null', () => {
    expect(mentionTokenAt('/kun done', 9)).toBeNull()
    expect(mentionTokenAt('', 0)).toBeNull()
  })
})

describe('filterSkills', () => {
  const list = [
    skill('kun-new-session'),
    { ...skill('pdf'), name: 'PDF Tools', description: 'extract pdf text' },
    skill('go-deploy')
  ]

  it('按 id 子串过滤', () => {
    expect(filterSkills(list, 'kun').map((s) => s.id)).toEqual(['kun-new-session'])
  })

  it('按名称与描述过滤，大小写不敏感', () => {
    expect(filterSkills(list, 'tools').map((s) => s.id)).toEqual(['pdf'])
    expect(filterSkills(list, 'EXTRACT').map((s) => s.id)).toEqual(['pdf'])
  })

  it('空查询返回全量副本', () => {
    expect(filterSkills(list, '  ')).toHaveLength(3)
  })
})
