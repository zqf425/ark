// @vitest-environment jsdom
/**
 * The reported bug, end to end: a turn that runs while the page is open (bare
 * tool_call_started/finished SSE events, no item snapshots) must still yield a
 * prose file chip in the final reply — threadWritePaths reads the folded item.
 */
import { describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../shared/contracts'
import { applyEvent, emptyItemsState, seedFromItems } from './state/items-reducer'
import { threadWritePaths } from './tool-file'
import { renderMarkdown } from '../../shared/ui/markdown-render'

const base = {
  seq: 1,
  thread_id: '00000000-0000-0000-0000-000000000000' as const,
  timestamp: '2026-11-16T01:00:00Z',
  auto_continuation: false,
  auto_recovery: 0,
  attachment_ids: [] as string[],
  attachment_outcomes: []
}
const turnId = '00000000-0000-0000-0000-0000000000t1' as const

describe('live turn → prose chip', () => {
  it('a write folded from bare events feeds the prose whitelist', () => {
    let state = emptyItemsState()
    state = applyEvent(state, { ...base, kind: 'turn_started', turn_id: turnId })
    state = applyEvent(state, {
      ...base, seq: 2, kind: 'tool_call_started', turn_id: turnId,
      item_id: '00000000-0000-0000-0000-0000000000b1',
      call_id: 'c1', tool: 'write', arguments: { path: '/ws/prose-chip-verify.md', content: 'x' }
    })
    state = applyEvent(state, {
      ...base, seq: 3, kind: 'tool_call_finished', turn_id: turnId,
      item_id: '00000000-0000-0000-0000-0000000000b1',
      call_id: 'c1', tool: 'write', ok: true, duration_ms: 4, output: 'wrote'
    })
    state = applyEvent(state, {
      ...base, seq: 4, kind: 'item_created', turn_id: turnId,
      item: {
        item_id: 'a1', turn_id: turnId, kind: 'assistant_message', status: 'completed',
        created_at: base.timestamp, text: '文档已生成:**`prose-chip-verify.md`**'
      }
    })

    const items = state.itemOrder.map((id) => state.itemsById[id]!) as ThreadItem[]
    const paths = threadWritePaths(items)
    expect(paths).toEqual(['/ws/prose-chip-verify.md'])

    const html = renderMarkdown('文档已生成:**`prose-chip-verify.md`**', paths)
    expect(html).toContain('ark-md-filechip')
    expect(html).toContain('data-md-file="/ws/prose-chip-verify.md"')
    // Detail seed merge must not duplicate the folded row: the server stores
    // the same deterministic id, so replaying the projection over the folded
    // state keeps one row per call.
    const seeded = seedFromItems(items, 4, { pending_approvals: [], pending_user_inputs: [] })
    expect(seeded.itemOrder.length).toBe(items.length)
  })
})
