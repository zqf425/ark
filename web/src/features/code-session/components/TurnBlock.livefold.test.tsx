// @vitest-environment jsdom
/**
 * The process card folds its earlier activity while the turn *runs*, not only
 * after it settles. A long turn must scan by its latest steps: the newest
 * `RECENT_ACTIVITY_ROW_LIMIT` reasoning/tool rows stay visible and everything
 * earlier folds behind one summary row.
 *
 * Settling is the other half of the contract: whatever the user expanded while
 * the turn ran, a finished turn folds down to its default so the canvas shows
 * the answer, not a wall of process rows (see `useEchoSafeToggle`).
 */
import { act } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { TurnBlock } from './TurnBlock'

installBrowserStubs()

const TURN = '00000000-0000-0000-000-0000000000c1'

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

/** One turn with five tool calls; the last row streams while `running`. */
function group(state: 'running' | 'completed'): {
  turnId: string
  state: 'running' | 'completed'
  items: ThreadItem[]
} {
  return {
    turnId: TURN,
    state,
    items: [
      item('u1', 'user_message', '继续'),
      item('t1', 'tool_call'),
      item('t2', 'tool_call'),
      item('t3', 'tool_call'),
      item('t4', 'tool_call'),
      item('t5', 'tool_call'),
      item('a1', 'assistant_message', '结论')
    ]
  }
}

async function flushToggleTasks(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

/** Tool cards currently rendered inside the turn (bare and structured alike). */
function renderedToolCards(host: HTMLElement): number {
  return host.querySelectorAll('.ark-tool').length
}

describe('TurnBlock folds earlier activity while the turn runs', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('keeps only the newest activity rows visible mid-turn', async () => {
    const harness = await mountWithProviders(
      <TurnBlock threadId="t" group={group('running')} availability="hidden" />
    )
    await flushToggleTasks()

    // Five tool calls, three visible: the two earliest fold away.
    const summary = harness.host.querySelector('.ark-foldedtools')
    expect(summary).not.toBeNull()
    expect(summary!.textContent).toContain('2')
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(3)
  })

  it('expands the folded rows on click and folds them back on a second click', async () => {
    const harness = await mountWithProviders(
      <TurnBlock threadId="t" group={group('running')} availability="hidden" />
    )
    await flushToggleTasks()

    const summary = harness.host.querySelector('.ark-foldedtools') as HTMLElement
    await act(async () => {
      summary.click()
      await Promise.resolve()
    })
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(5)

    await act(async () => {
      ;(harness.host.querySelector('.ark-foldedtools') as HTMLElement).click()
      await Promise.resolve()
    })
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(3)
  })

  it('drops the mid-turn expansion when the turn settles', async () => {
    const harness = await mountWithProviders(
      <TurnBlock threadId="t" group={group('running')} availability="hidden" />
    )
    await flushToggleTasks()

    // The user expands the whole trace while the turn runs.
    await act(async () => {
      ;(harness.host.querySelector('.ark-foldedtools') as HTMLElement).click()
      await Promise.resolve()
    })
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(5)

    // Settling collapses the whole process card: the answer, not the trace.
    await harness.render(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    await flushToggleTasks()
    const details = harness.host.querySelector('details')!
    expect(details.hasAttribute('open')).toBe(false)
    // The final reply was promoted out of the card into its own answer card,
    // and the trace behind the closed card is back on its folded default.
    expect(harness.host.querySelector('.ark-reply')).not.toBeNull()
    expect(renderedToolCards(harness.host)).toBe(3)
  })

  it('reopens a settled turn on the folded default, not the mid-turn expansion', async () => {
    const harness = await mountWithProviders(
      <TurnBlock threadId="t" group={group('running')} availability="hidden" />
    )
    await flushToggleTasks()
    await act(async () => {
      ;(harness.host.querySelector('.ark-foldedtools') as HTMLElement).click()
      await Promise.resolve()
    })
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(5)

    await harness.render(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    await flushToggleTasks()

    // Reopening the finished card is a settled-phase action: the trace starts
    // folded again, so browsing a history turn never dumps every row at once.
    await act(async () => {
      ;(harness.host.querySelector('summary') as HTMLElement).dispatchEvent(
        new window.MouseEvent('click', { bubbles: true, cancelable: true })
      )
      await Promise.resolve()
    })
    await flushToggleTasks()
    expect(harness.host.querySelector('details')!.hasAttribute('open')).toBe(true)
    expect(harness.host.querySelectorAll('.ark-tool').length).toBe(3)
  })

  it('does not fold a turn with three or fewer activity rows', async () => {
    const short = {
      turnId: TURN,
      state: 'running' as const,
      items: [item('u1', 'user_message', 'hi'), item('t1', 'tool_call'), item('r1', 'reasoning')]
    }
    const harness = await mountWithProviders(
      <TurnBlock threadId="t" group={short} availability="hidden" />
    )
    await flushToggleTasks()
    expect(harness.host.querySelector('.ark-foldedtools')).toBeNull()
  })
})
