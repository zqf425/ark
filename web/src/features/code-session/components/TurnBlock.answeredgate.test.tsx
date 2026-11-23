// @vitest-environment jsdom
/**
 * An answered input gate must fold into its own turn as a quiet summary row —
 * the settled conversation continues from the turn's answer, and a stack of
 * "需要你的输入 / 已回答" gate cards must never re-pin itself below the
 * newest turn (the canvas follows the newest row, so those cards sat in the
 * reader's face forever).
 */
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { TurnBlock } from './TurnBlock'

installBrowserStubs()

const TURN = '00000000-0000-0000-0000-0000000000b2'

function item(id: string, kind: ThreadItem['kind'], text?: string, detail?: string): ThreadItem {
  return {
    item_id: id,
    turn_id: TURN,
    kind,
    status: 'completed',
    created_at: '2026-11-14T08:00:00Z',
    ...(text === undefined ? {} : { text }),
    ...(detail === undefined ? {} : { detail })
  } as ThreadItem
}

function group(items: ThreadItem[]): { turnId: string; state: 'completed'; items: ThreadItem[] } {
  return { turnId: TURN, state: 'completed', items }
}

describe('TurnBlock folds an answered input gate into its turn', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('renders the answered gate as one collapsed summary row inside the turn', async () => {
    const harness = await mountWithProviders(
      <TurnBlock
        threadId="t"
        group={group([
          item('u1', 'user_message', '继续'),
          item('g1', 'user_input', '选哪个分支？', 'main'),
          item('a1', 'assistant_message', '已切换到 main')
        ])}
        availability="hidden"
      />
    )

    const row = harness.host.querySelector<HTMLDetailsElement>('details.ark-answeredgate')
    expect(row).not.toBeNull()
    expect(row!.open).toBe(false)
    expect(harness.text()).toContain('已回答')
    // The verbatim answer only lives in the collapsible body, never in the
    // summary line (jsdom textContent ignores CSS folding, so visibility is
    // asserted per section, not on the whole host).
    const summary = row!.querySelector('.ark-answeredgate__head')
    expect(summary!.textContent).not.toContain('main')
    const body = row!.querySelector('.ark-answeredgate__body')
    expect(body!.textContent).toContain('main')
    await harness.unmount()
  })

  it('keeps turns without an answered gate free of the summary row', async () => {
    const harness = await mountWithProviders(
      <TurnBlock
        threadId="t"
        group={group([item('u1', 'user_message', '继续'), item('a1', 'assistant_message', '结论')])}
        availability="hidden"
      />
    )

    expect(harness.host.querySelector('.ark-answeredgate')).toBeNull()
    await harness.unmount()
  })
})
