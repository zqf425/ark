// @vitest-environment jsdom
/**
 * The turn's measured model time renders in the **process-card header**, not in
 * the reply card's bottom-right corner: the header is the row describing what
 * the turn did, so its cost belongs there.
 *
 * A step-less turn (a single question and answer) renders no process card, so
 * that turn's clock rides the reply card's title row instead — the duration
 * must never disappear just because nothing folded.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { TurnBlock } from './TurnBlock'

installBrowserStubs()

const TURN = '00000000-0000-0000-0000-0000000000a1'

function item(id: string, kind: ThreadItem['kind'], text?: string): ThreadItem {
  return {
    item_id: id,
    turn_id: TURN,
    kind,
    status: 'completed',
    created_at: '2026-11-17T07:10:00Z',
    ...(text === undefined ? {} : { text })
  } as ThreadItem
}

function group(items: ThreadItem[]): { turnId: string; state: 'completed'; items: ThreadItem[] } {
  return { turnId: TURN, state: 'completed', items }
}

/** Only this turn has rounds; the clock is 1m 41s. */
function seedRounds(): void {
  useCodeSessionStore.setState({ rounds: [{ turnId: TURN, durationMs: 101_000 }] })
}

describe('TurnBlock renders the turn duration in the process header', () => {
  beforeEach(seedRounds)

  afterEach(() => {
    useCodeSessionStore.setState({ rounds: [] })
    document.body.innerHTML = ''
  })

  it('puts the clock in the process-card header, not the reply footer', async () => {
    const harness = await mountWithProviders(
      <TurnBlock
        threadId="t"
        group={group([item('u1', 'user_message', '继续'), item('t1', 'tool_call'), item('a1', 'assistant_message', '结论')])}
        availability="hidden"
      />
    )

    const header = harness.host.querySelector('.ark-process__head')
    expect(header).not.toBeNull()
    const headerClock = header!.querySelector('.ark-process__duration')
    expect(headerClock).not.toBeNull()
    expect(headerClock!.textContent).toContain('1:41')

    // The reply card carries no clock at all when a process card exists.
    expect(harness.host.querySelector('.ark-reply__duration')).toBeNull()
    expect(harness.host.querySelector('.ark-reply__foot')?.textContent ?? '').not.toContain('1:41')
  })

  it('falls back to the reply title row when the turn has no process card', async () => {
    const harness = await mountWithProviders(
      <TurnBlock
        threadId="t"
        group={group([item('u1', 'user_message', '你好'), item('a1', 'assistant_message', '你好，有什么可以帮你？')])}
        availability="hidden"
      />
    )

    expect(harness.host.querySelector('.ark-process__head')).toBeNull()
    const titleRow = harness.host.querySelector('.ark-reply__head')
    expect(titleRow).not.toBeNull()
    const clock = titleRow!.querySelector('.ark-reply__duration')
    expect(clock).not.toBeNull()
    expect(clock!.textContent).toContain('1:41')
    // Still not the bottom-right action row.
    expect(harness.host.querySelector('.ark-reply__foot')?.querySelector('.ark-reply__duration')).toBeNull()
  })
})
