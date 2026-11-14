// @vitest-environment jsdom
/**
 * The process card must auto-collapse when its turn settles, and a user
 * toggle must still pin it. The failure mode was the native `toggle` echo:
 * the HTML spec dispatches the event asynchronously for *any* open-state
 * transition, including React's own controlled writes and the mount, so the
 * old `onToggle` pinned the card open the moment it mounted and a settled
 * turn never collapsed (mainstream UIs avoid this by writing state only
 * from explicit user-interaction callbacks — Radix trigger, button click).
 *
 * `useEchoSafeToggle` filters echoes by comparing the reported DOM state
 * against the value React last rendered. jsdom implements both the async
 * toggle dispatch and the `<summary>` activation behavior, so these tests
 * drive the real event path.
 */
import { act } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { TurnBlock } from './TurnBlock'

installBrowserStubs()

const TURN = '00000000-0000-0000-000-0000000000b1'

function item(id: string, kind: ThreadItem['kind'], text?: string): ThreadItem {
  return {
    item_id: id,
    turn_id: TURN,
    kind,
    status: 'completed',
    created_at: '2026-11-14T08:00:00Z',
    ...(text === undefined ? {} : { text })
  } as ThreadItem
}

function group(state: 'running' | 'completed') {
  return {
    turnId: TURN,
    state,
    items: [item('u1', 'user_message', '继续'), item('r1', 'reasoning', '想一想')] as ThreadItem[]
  }
}

async function flushToggleTasks(): Promise<void> {
  // jsdom queues the toggle event in a `setTimeout(0)` task; flushing a real
  // macrotask inside act settles the event and the state update it schedules.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

describe('TurnBlock process card auto-collapse', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('auto-collapses when the turn settles (mount echo does not pin it open)', async () => {
    const harness = await mountWithProviders(<TurnBlock threadId="t" group={group('running')} availability="hidden" />)
    const details = harness.host.querySelector('details')
    expect(details).not.toBeNull()
    expect(details!.hasAttribute('open')).toBe(true)

    // Mount echo: the async toggle fires for the initial open attribute.
    await flushToggleTasks()

    // turn_completed arrives → live flips false.
    await harness.render(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    await flushToggleTasks()

    expect(details!.hasAttribute('open')).toBe(false)
  })

  it('stays closed for a turn that settled before mount', async () => {
    const harness = await mountWithProviders(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    const details = harness.host.querySelector('details')
    await flushToggleTasks()
    expect(details!.hasAttribute('open')).toBe(false)
  })

  it('a real user toggle pins the card open across settling', async () => {
    const harness = await mountWithProviders(<TurnBlock threadId="t" group={group('running')} availability="hidden" />)
    const summary = harness.host.querySelector('summary')!
    await flushToggleTasks()

    // User closes it mid-turn: genuine interaction → pinned closed.
    await act(async () => {
      summary.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true }))
      await Promise.resolve()
    })
    await flushToggleTasks()

    const details = harness.host.querySelector('details')!
    expect(details.hasAttribute('open')).toBe(false)

    // The turn settles while the user pin is in force: it must stay closed.
    await harness.render(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    await flushToggleTasks()
    expect(details.hasAttribute('open')).toBe(false)
  })

  it('a user toggle reopens a settled card and the pin holds', async () => {
    const harness = await mountWithProviders(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    await flushToggleTasks()
    const summary = harness.host.querySelector('summary')!

    await act(async () => {
      summary.dispatchEvent(new window.MouseEvent('click', { bubbles: true, cancelable: true }))
      await Promise.resolve()
    })
    await flushToggleTasks()

    expect(harness.host.querySelector('details')!.hasAttribute('open')).toBe(true)
  })
})
