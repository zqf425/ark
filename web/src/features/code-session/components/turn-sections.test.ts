import { describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import {
  finalReplyIndex,
  isActivityRow,
  isFoldedProcessRow,
  splitTurnSections,
  splitProcessTrace,
  RECENT_ACTIVITY_ROW_LIMIT
} from './turn-sections'

function item(id: string, kind: ThreadItem['kind'], text?: string): ThreadItem {
  return {
    item_id: id,
    turn_id: 'turn-1',
    kind,
    status: 'completed',
    created_at: '2026-01-01T00:00:00Z',
    ...(text === undefined ? {} : { text })
  } as ThreadItem
}

describe('finalReplyIndex', () => {
  it('returns -1 while the turn runs: nothing is final yet', () => {
    const rows = [item('u', 'user_message', 'hi'), item('a', 'assistant_message', 'partial')]
    expect(finalReplyIndex(rows, 'running')).toBe(-1)
  })

  it('picks the last assistant message with visible text', () => {
    const rows = [
      item('a1', 'assistant_message', '先看看'),
      item('t', 'tool_call'),
      item('a2', 'assistant_message', '最终回复'),
      item('a3', 'assistant_message', '   ')
    ]
    expect(finalReplyIndex(rows, 'settled')).toBe(2)
  })

  it('returns -1 when no assistant text exists', () => {
    const rows = [item('u', 'user_message', 'hi'), item('t', 'tool_call')]
    expect(finalReplyIndex(rows, 'settled')).toBe(-1)
  })
})

describe('splitTurnSections', () => {
  it('keeps the real event order inside the process card', () => {
    const rows = [
      item('u', 'user_message', '继续'),
      item('a1', 'assistant_message', '我先看看现状'),
      item('t1', 'tool_call'),
      item('t2', 'tool_call'),
      item('a2', 'assistant_message', '结论如下')
    ]
    const { user, process, reply } = splitTurnSections(rows, 'settled')
    expect(user.map((row) => row.item_id)).toEqual(['u'])
    expect(process.map((row) => row.item_id)).toEqual(['a1', 't1', 't2'])
    expect(reply.map((row) => row.item_id)).toEqual(['a2'])
  })

  it('routes every row to process while the turn runs', () => {
    const rows = [item('u', 'user_message', 'hi'), item('a', 'assistant_message', 'partial'), item('t', 'tool_call')]
    const { user, process, reply } = splitTurnSections(rows, 'running')
    expect(user.map((row) => row.item_id)).toEqual(['u'])
    expect(process.map((row) => row.item_id)).toEqual(['a', 't'])
    expect(reply).toEqual([])
  })

  it('keeps a pure chat turn outside the process card', () => {
    const rows = [item('u', 'user_message', 'hi'), item('a', 'assistant_message', 'hello')]
    const { process, reply } = splitTurnSections(rows, 'settled')
    expect(process).toEqual([])
    expect(reply.map((row) => row.item_id)).toEqual(['a'])
  })
})

describe('isActivityRow', () => {
  it('counts reasoning, tool calls and tool results as activity', () => {
    expect(isActivityRow(item('r', 'reasoning'))).toBe(true)
    expect(isActivityRow(item('t', 'tool_call'))).toBe(true)
    expect(isActivityRow(item('o', 'tool_result'))).toBe(true)
  })

  it('treats narration, errors and notices as non-activity rows', () => {
    expect(isActivityRow(item('n', 'assistant_message', 'hi'))).toBe(false)
    expect(isActivityRow(item('e', 'error'))).toBe(false)
    expect(isActivityRow(item('x', 'notice'))).toBe(false)
  })
})

describe('isFoldedProcessRow', () => {
  it('folds activity and mid-turn narration: the summary row stands for the whole earlier process', () => {
    expect(isFoldedProcessRow(item('r', 'reasoning'))).toBe(true)
    expect(isFoldedProcessRow(item('t', 'tool_call'))).toBe(true)
    expect(isFoldedProcessRow(item('o', 'tool_result'))).toBe(true)
    expect(isFoldedProcessRow(item('n', 'assistant_message', 'earlier narration'))).toBe(true)
  })

  it('never hides a failure or a notice behind the fold', () => {
    // Errors render at the interruption point by contract (docs/modules/web.md),
    // so folding must not swallow them.
    expect(isFoldedProcessRow(item('e', 'error'))).toBe(false)
    expect(isFoldedProcessRow(item('x', 'notice'))).toBe(false)
  })

  it('never hides a user message behind the fold', () => {
    // The user's own words are the human side of the conversation: in the
    // child-run viewer (no turn grouping) delegation texts and steering rows
    // land inside the trace, and the fold must not swallow them - the same
    // reason the parent card splits user messages out before folding.
    expect(isFoldedProcessRow(item('u', 'user_message', 'run the checks'))).toBe(false)
  })
})

describe('splitProcessTrace (default fold of earlier activity)', () => {
  it('keeps the newest activity rows in the tail and counts the hidden ones', () => {
    const rows = [
      item('t1', 'tool_call'),
      item('t2', 'tool_call'),
      item('t3', 'tool_call'),
      item('t4', 'tool_call'),
      item('t5', 'tool_call')
    ]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    expect(view.foldedActivityRows).toBe(2)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['t1', 't2'])
    expect(view.tail.map((row) => row.item_id)).toEqual(['t3', 't4', 't5'])
  })

  it('folds earlier reasoning and tools together while showing the latest activity', () => {
    const rows = [
      item('r1', 'reasoning', 'first thought'),
      item('t1', 'tool_call'),
      item('n1', 'assistant_message', 'intermediate narration'),
      item('r2', 'reasoning', 'second thought'),
      item('t2', 'tool_call'),
      item('r3', 'reasoning', 'latest thought')
    ]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    expect(view.foldedActivityRows).toBe(2)
    // Interleaved narration stays on the side of the fold point it happened
    // on, so the summary row reads as "earlier steps", not a rewritten order.
    expect(view.prefix.map((row) => row.item_id)).toEqual(['r1', 't1', 'n1'])
    expect(view.tail.map((row) => row.item_id)).toEqual(['r2', 't2', 'r3'])
  })

  it('keeps whole tool exchanges on the same side of the fold point', () => {
    const rows = [
      item('r1', 'reasoning', 'first thought'),
      item('t1', 'tool_call'),
      item('o1', 'tool_result', 'output one'),
      item('r2', 'reasoning', 'second thought'),
      item('t2', 'tool_call'),
      item('o2', 'tool_result', 'output two')
    ]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    // Six activity rows, three kept: the earlier call keeps its own result.
    expect(view.foldedActivityRows).toBe(3)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['r1', 't1', 'o1'])
    expect(view.tail.map((row) => row.item_id)).toEqual(['r2', 't2', 'o2'])
  })

  it('passes a trace with at most three activity rows through unchanged', () => {
    const rows = [item('r1', 'reasoning'), item('t1', 'tool_call'), item('r2', 'reasoning')]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    expect(view.foldedActivityRows).toBe(0)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['r1', 't1', 'r2'])
    expect(view.tail).toEqual([])
  })

  it('does not create a fold for a trace without activity rows', () => {
    const rows = [item('n1', 'assistant_message', 'thinking aloud'), item('n2', 'notice', 'notice')]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    expect(view.foldedActivityRows).toBe(0)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['n1', 'n2'])
  })

  it('keeps the streaming row visible: the newest activity row is always in the tail', () => {
    const rows = [
      item('t1', 'tool_call'),
      item('t2', 'tool_call'),
      item('t3', 'tool_call'),
      { ...item('r9', 'reasoning'), status: 'streaming' as const }
    ]
    const view = splitProcessTrace(rows, RECENT_ACTIVITY_ROW_LIMIT)
    expect(view.foldedActivityRows).toBe(1)
    expect(view.tail.map((row) => row.item_id)).toEqual(['t2', 't3', 'r9'])
    expect(view.tail.at(-1)!.status).toBe('streaming')
  })
})
