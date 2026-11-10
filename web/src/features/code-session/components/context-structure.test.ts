import { describe, expect, it } from 'vitest'
import type { SkillSummary, ThreadItem } from '../../../shared/contracts'
import { CONTEXT_CATEGORY_ORDER, contextStructure, type ContextCategoryKey } from './context-structure'

const TURN_ID = '00000000-0000-0000-0000-000000000001'

function item(id: string, kind: ThreadItem['kind'], overrides: Partial<ThreadItem> = {}): ThreadItem {
  return {
    item_id: id,
    turn_id: TURN_ID,
    kind,
    status: 'completed',
    created_at: '2026-01-01T00:00:00Z',
    ...overrides
  }
}

function skill(): SkillSummary {
  return { id: 'review', name: 'Review', description: 'Check a diff', version: '1', root: '/skills', entry_path: 'SKILL.md', priority: 1, source: 'base', legacy: false }
}

function segmentOf(items: readonly ThreadItem[], key: ContextCategoryKey, skills: readonly SkillSummary[] = []) {
  return contextStructure(items, skills).segments.find((segment) => segment.key === key)
}

describe('contextStructure', () => {
  it('groups render rows and loaded skills into stable categories', () => {
    const rows = [
      item('1', 'user_message', { text: 'hello' }),
      item('2', 'assistant_message', { text: 'world' }),
      item('3', 'tool_call', { tool: { call_id: 'c1', tool: 'read', output: 'abcd' } }),
      item('4', 'tool_result', { text: 'result' }),
      item('5', 'reasoning', { text: 'think' }),
      item('6', 'notice', { text: 'a notice' }),
      item('7', 'notice', { text: 'checkpoint', marker: 'context_checkpoint' }),
      item('8', 'error', { text: 'boom' })
    ]
    const structure = contextStructure(rows, [skill()])
    expect(structure.totalItems).toBe(9)
    expect(segmentOf(rows, 'messages')?.items).toBe(2)
    expect(segmentOf(rows, 'tools')?.items).toBe(2)
    expect(segmentOf(rows, 'reasoning')?.items).toBe(1)
    expect(segmentOf(rows, 'system')?.items).toBe(1)
    expect(segmentOf(rows, 'checkpoint')?.items).toBe(1)
    expect(segmentOf(rows, 'other')?.items).toBe(1)
    expect(segmentOf(rows, 'skills', [skill()])?.items).toBe(1)
  })

  it('splits notices by the structured marker instead of text prefixes', () => {
    const rows = [item('1', 'notice', { text: '[context checkpoint]\nGoal: x' })]
    expect(segmentOf(rows, 'system')?.items).toBe(1)
    expect(segmentOf(rows, 'checkpoint')?.items).toBe(0)
  })

  it('counts text, tool output, detail and skill metadata as visible chars', () => {
    const rows = [item('1', 'tool_call', { text: 'abc', detail: 'de', tool: { call_id: 'c1', tool: 'read', output: 'fghi' } })]
    expect(segmentOf(rows, 'tools')?.chars).toBe(9)
    expect(segmentOf([], 'skills', [skill()])?.chars).toBeGreaterThan(0)
  })

  it('produces ratios that sum to one and stays empty for an empty thread', () => {
    const empty = contextStructure([])
    expect(empty.totalChars).toBe(0)
    expect(empty.segments.every((segment) => segment.ratio === 0)).toBe(true)
    const rows = [item('1', 'user_message', { text: 'abcd' }), item('2', 'tool_call', { text: 'efgh' })]
    const structure = contextStructure(rows)
    const sum = structure.segments.reduce((total, segment) => total + segment.ratio, 0)
    expect(sum).toBeCloseTo(1)
    expect(structure.segments).toHaveLength(CONTEXT_CATEGORY_ORDER.length)
  })
})
