// @vitest-environment jsdom
/**
 * A compaction checkpoint is model-facing handoff prose (often thousands of
 * chars of failure evidence after a long session). It must surface as one
 * quiet collapsed line — never as raw text in the stream, which a user
 * resuming a session would read as a runtime error.
 */
import { describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { NoticeRow } from './ErrorRow'

function checkpointItem(text: string): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000c1',
    turn_id: '00000000-0000-0000-0000-0000000000t1',
    kind: 'notice',
    status: 'completed',
    created_at: '2026-10-28T00:00:00Z',
    text,
    marker: 'context_checkpoint'
  }
}

describe('checkpoint rendering', () => {
  it('collapses a checkpoint into a compact summary line', async () => {
    installBrowserStubs()
    const harness = await mountWithProviders(
      <NoticeRow item={checkpointItem('[context checkpoint]\n[user requests]\n- 卡住了嘛？')} />
    )

    const details = harness.host.querySelector<HTMLDetailsElement>("details.ark-checkpoint")
    expect(details).not.toBeNull()
    // Collapsed by default: the handoff prose is hidden until expanded.
    expect(details!.open).toBe(false)
    expect(harness.text()).toContain('上下文已压缩')
    await harness.unmount()
  })

  it('reports how many earlier messages were folded', async () => {
    installBrowserStubs()
    const harness = await mountWithProviders(
      <NoticeRow
        item={checkpointItem('[context checkpoint]\n[438 earlier messages compacted]')}
      />
    )

    expect(harness.text()).toContain('已折叠 438 条较早消息')
    await harness.unmount()
  })

  it('keeps the handoff text reachable when expanded', async () => {
    installBrowserStubs()
    const harness = await mountWithProviders(
      <NoticeRow item={checkpointItem('[context checkpoint]\nGoal: preserve legacy fact')} />
    )
    const details = harness.host.querySelector<HTMLDetailsElement>("details.ark-checkpoint")
    expect(details).not.toBeNull()
    details!.open = true
    await harness.render(
      <NoticeRow item={checkpointItem('[context checkpoint]\nGoal: preserve legacy fact')} />
    )

    expect(harness.text()).toContain('Goal: preserve legacy fact')
    await harness.unmount()
  })

  it('renders a plain notice without the marker verbatim as before', async () => {
    installBrowserStubs()
    const harness = await mountWithProviders(
      <NoticeRow
        item={{
          item_id: '00000000-0000-0000-0000-0000000000c2',
          turn_id: '00000000-0000-0000-0000-0000000000t2',
          kind: 'notice',
          status: 'completed',
          created_at: '2026-10-28T00:00:00Z',
          text: '普通通知'
        }}
      />
    )

    expect(harness.text()).toContain('普通通知')
    expect(harness.host.querySelector('details.ark-checkpoint')).toBeNull()
    await harness.unmount()
  })
})
