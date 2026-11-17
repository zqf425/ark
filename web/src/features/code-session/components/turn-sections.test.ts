import { describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { finalReplyIndex, splitTurnSections, splitProcessTrace, RECENT_TOOL_ROW_LIMIT } from './turn-sections'

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
    // Narration before the tools stays BEFORE the tools — the bug was that
    // every tool jumped ahead of every reply.
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

describe('splitProcessTrace (default fold of older tool rows)', () => {
  it('keeps the newest tool calls in the tail and counts the folded ones', () => {
    const rows = [
      item('t1', 'tool_call'),
      item('t2', 'tool_call'),
      item('t3', 'tool_call'),
      item('t4', 'tool_call'),
      item('t5', 'tool_call')
    ]
    const view = splitProcessTrace(rows, RECENT_TOOL_ROW_LIMIT)
    expect(view.foldedToolCalls).toBe(2)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['t1', 't2'])
    expect(view.tail.map((row) => row.item_id)).toEqual(['t3', 't4', 't5'])
  })

  it('keeps non-tool rows on the side of the fold point they belong to', () => {
    const rows = [
      item('t1', 'tool_call'),
      item('n1', 'assistant_message', '中间叙述'),
      item('t2', 'tool_call'),
      item('t3', 'tool_call'),
      item('t4', 'tool_call')
    ]
    const view = splitProcessTrace(rows, RECENT_TOOL_ROW_LIMIT)
    expect(view.foldedToolCalls).toBe(1)
    // The narration happened before the first kept tool call, so it stays in
    // the prefix; the summary row renders between prefix and tail.
    expect(view.prefix.map((row) => row.item_id)).toEqual(['t1', 'n1'])
    expect(view.tail.map((row) => row.item_id)).toEqual(['t2', 't3', 't4'])
  })

  it('passes a short trace through unchanged', () => {
    const rows = [item('t1', 'tool_call'), item('t2', 'tool_call'), item('t3', 'tool_call')]
    const view = splitProcessTrace(rows, RECENT_TOOL_ROW_LIMIT)
    expect(view.foldedToolCalls).toBe(0)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['t1', 't2', 't3'])
    expect(view.tail).toEqual([])
  })

  it('handles a trace without any tool rows', () => {
    const rows = [item('n1', 'reasoning'), item('n2', 'assistant_message', '想')]
    const view = splitProcessTrace(rows, RECENT_TOOL_ROW_LIMIT)
    expect(view.foldedToolCalls).toBe(0)
    expect(view.prefix.map((row) => row.item_id)).toEqual(['n1', 'n2'])
  })
})
