/**
 * Pure folding rules for one child transcript: the snapshot seed, the delta
 * append (including the delta-wins-the-race materialization), the watermark
 * guard and the row-carrying event filter.
 */
import { describe, expect, it } from 'vitest'
import type { RuntimeEvent, ThreadItem } from '../../../shared/contracts'
import { foldChildEvent, seedChildTranscript } from './child-transcript'

const CHILD_THREAD = '00000000-0000-0000-0000-0000000000c1'
const TURN = '00000000-0000-0000-0000-0000000000t1'

function item(overrides: Partial<ThreadItem>): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000i1',
    turn_id: TURN,
    kind: 'assistant_message',
    status: 'completed',
    created_at: '2026-10-28T00:00:00Z',
    ...overrides
  }
}

describe('seedChildTranscript', () => {
  it('seeds rows in server order and adopts the watermark', () => {
    const state = seedChildTranscript([item({}), item({ item_id: '00000000-0000-0000-0000-0000000000i2' })], 41)
    expect(state.itemOrder).toEqual([
      '00000000-0000-0000-0000-0000000000i1',
      '00000000-0000-0000-0000-0000000000i2'
    ])
    expect(state.lastSeq).toBe(41)
    expect(state.status).toBe('ready')
  })
})

describe('foldChildEvent', () => {
  it('appends a delta to the streamed item without duplicating the row', () => {
    let state = seedChildTranscript([item({ status: 'streaming', text: 'he' })], 5)
    state = foldChildEvent(state, {
      kind: 'assistant_text_delta',
      seq: 6,
      thread_id: CHILD_THREAD,
      turn_id: TURN,
      item_id: '00000000-0000-0000-0000-0000000000i1',
      timestamp: '2026-10-28T00:00:01Z',
      delta: 'llo'
    })
    expect(state.itemOrder).toHaveLength(1)
    expect(state.itemsById['00000000-0000-0000-0000-0000000000i1']?.text).toBe('hello')
    expect(state.lastSeq).toBe(6)
  })

  it('materializes a row when a delta wins the race over item_created', () => {
    const state = foldChildEvent(seedChildTranscript([], 1), {
      kind: 'assistant_reasoning_delta',
      seq: 2,
      thread_id: CHILD_THREAD,
      turn_id: TURN,
      item_id: '00000000-0000-0000-0000-0000000000i9',
      timestamp: '2026-10-28T00:00:01Z',
      delta: 'think'
    })
    const row = state.itemsById['00000000-0000-0000-0000-0000000000i9']
    expect(row?.kind).toBe('reasoning')
    expect(row?.status).toBe('streaming')
    expect(state.itemOrder).toEqual(['00000000-0000-0000-0000-0000000000i9'])
  })

  it('ignores frames at or below the watermark (replay after reconnect)', () => {
    const state = seedChildTranscript([item({ text: 'kept' })], 10)
    const replayed = foldChildEvent(state, {
      kind: 'assistant_text_delta',
      seq: 10,
      thread_id: CHILD_THREAD,
      turn_id: TURN,
      item_id: '00000000-0000-0000-0000-0000000000i1',
      timestamp: '2026-10-28T00:00:01Z',
      delta: ' AGAIN'
    })
    expect(replayed).toBe(state)
  })

  it('upserts item_completed without reordering rows', () => {
    let state = seedChildTranscript([item({ status: 'streaming' })], 5)
    state = foldChildEvent(state, {
      kind: 'item_completed',
      seq: 7,
      thread_id: CHILD_THREAD,
      turn_id: TURN,
      timestamp: '2026-10-28T00:00:02Z',
      item: item({ status: 'completed', text: 'final' })
    })
    expect(state.itemOrder).toHaveLength(1)
    expect(state.itemsById['00000000-0000-0000-0000-0000000000i1']?.status).toBe('completed')
    expect(state.itemsById['00000000-0000-0000-0000-0000000000i1']?.text).toBe('final')
  })

  it('lifecycle events only move the watermark', () => {
    const state = seedChildTranscript([], 3)
    const kinds: RuntimeEvent['kind'][] = ['turn_started', 'turn_completed', 'child_activity']
    for (const [index, kind] of kinds.entries()) {
      const next = foldChildEvent(state, {
        kind,
        seq: 4 + index,
        thread_id: CHILD_THREAD,
        turn_id: TURN,
        timestamp: '2026-10-28T00:00:03Z'
      } as RuntimeEvent)
      expect(next.itemOrder).toHaveLength(0)
      expect(next.lastSeq).toBe(4 + index)
    }
  })
})
