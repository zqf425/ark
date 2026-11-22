// @vitest-environment jsdom
/**
 * The turn's measured model time renders in the **process-card header**, right
 * after the step count and not pinned to the row's right edge: it reads as
 * part of the summary line, and the reply card's bottom-right corner stays
 * free for the copy action.
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

/** Only this turn has clocks; the active span is 1m 41s. */
function seedRounds(): void {
  useCodeSessionStore.setState({
    rounds: [{ turnId: TURN, durationMs: 101_000, activeMs: 101_000 }],
    turnLastEventAt: {}
  })
}

describe('TurnBlock renders the turn duration in the process header', () => {
  beforeEach(seedRounds)

  afterEach(() => {
    useCodeSessionStore.setState({ rounds: [], turnLastEventAt: {} })
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
    // Spelled-out units, not a `m:ss` clock.
    expect(headerClock!.textContent).toBe('耗时 1分41秒')

    // It follows the step count directly, so the summary line reads
    // "执行过程 · 1 步 · 耗时 1分41秒" and the chevron keeps the right edge.
    const stepCount = header!.querySelector('.ark-process__meta')
    expect(stepCount).not.toBeNull()
    expect(stepCount!.nextElementSibling).toBe(headerClock)

    // The reply card carries no clock at all when a process card exists.
    expect(harness.host.querySelector('.ark-reply__duration')).toBeNull()
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
    expect(clock!.textContent).toBe('耗时 1分41秒')
    // Still not the bottom-right action row.
    expect(harness.host.querySelector('.ark-reply__foot')?.querySelector('.ark-reply__duration')).toBeNull()
  })
})
