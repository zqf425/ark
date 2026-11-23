// @vitest-environment jsdom
/**
 * An answered user-input gate is history, not a live control: it must render
 * as one collapsed summary line inside its turn (never the full gate card
 * pinned under the newest turn), and expanding reveals the verbatim prompt
 * and answer.
 */
import { act } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { AnsweredGateRow } from './AnsweredGateRow'

installBrowserStubs()

function gateItem(): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000g1',
    turn_id: '00000000-0000-0000-0000-0000000000t1',
    kind: 'user_input',
    status: 'completed',
    created_at: '2026-11-14T08:00:00Z',
    text: '选择一个分支名？',
    detail: 'main'
  } as ThreadItem
}

describe('AnsweredGateRow', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('collapses to one summary line by default', async () => {
    const harness = await mountWithProviders(<AnsweredGateRow item={gateItem()} />)

    const row = harness.host.querySelector<HTMLDetailsElement>('details.ark-answeredgate')
    expect(row).not.toBeNull()
    expect(row!.open).toBe(false)
    expect(harness.text()).toContain('已回答')
    expect(harness.text()).toContain('选择一个分支名？')
    // The verbatim answer only lives in the collapsible body, never in the
    // summary line (jsdom textContent ignores CSS folding, so visibility is
    // asserted per section, not on the whole host).
    const summary = row!.querySelector('.ark-answeredgate__head')
    expect(summary!.textContent).not.toContain('main')
    const body = row!.querySelector('.ark-answeredgate__body')
    expect(body!.textContent).toContain('main')
    await harness.unmount()
  })

  it('reveals the prompt and answer when expanded', async () => {
    const harness = await mountWithProviders(<AnsweredGateRow item={gateItem()} />)

    await act(async () => {
      const head = harness.host.querySelector<HTMLDivElement>('.ark-answeredgate__head')
      head!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await Promise.resolve()
    })

    const details = harness.host.querySelector<HTMLDetailsElement>('details.ark-answeredgate')!
    expect(details.open).toBe(true)
    // Expanded: prompt and answer both visible.
    expect(harness.text()).toContain('选择一个分支名？')
    expect(harness.text()).toContain('main')
    await harness.unmount()
  })
})
