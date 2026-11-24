// @vitest-environment jsdom
/**
 * Reasoning rows read as timeline entries: a settled block summarizes itself
 * as "已思考 · <one-line preview>" (Qoder-referenced layout) instead of a bare
 * "思考过程" label, and a streaming one says "思考中". The full text stays one
 * click away in the fold body.
 */
import { afterEach, describe, expect, it } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { ReasoningBlock } from './MessageBubble'

installBrowserStubs()

function reasoning(text: string, status: ThreadItem['status'] = 'completed'): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000r1',
    turn_id: '00000000-0000-0000-0000-0000000000t1',
    kind: 'reasoning',
    status,
    created_at: '2026-11-20T00:00:00Z',
    text
  }
}

describe('ReasoningBlock timeline summary', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('summarizes a settled block as 已思考 with a one-line preview', async () => {
    const h = await mountWithProviders(<ReasoningBlock item={reasoning('先检查目录结构\n再决定改动范围')} />)
    const summary = h.host.querySelector('.ark-reasoning__summary')!
    expect(summary.textContent).toContain('已思考')
    const preview = summary.querySelector('.ark-reasoning__preview')
    expect(preview).not.toBeNull()
    // Newlines collapse so the summary stays one line.
    expect(preview!.textContent).toBe('先检查目录结构 再决定改动范围')
    // The full text is still in the fold body, verbatim.
    expect(h.host.querySelector('.ark-reasoning__body')!.textContent).toBe('先检查目录结构\n再决定改动范围')
  })

  it('says 思考中 while the block streams', async () => {
    const h = await mountWithProviders(<ReasoningBlock item={reasoning('正在权衡', 'streaming')} />)
    const summary = h.host.querySelector('.ark-reasoning__summary')!
    expect(summary.textContent).toContain('思考中')
    expect(summary.textContent).not.toContain('已思考')
  })

  it('renders no preview span for an empty block', async () => {
    const h = await mountWithProviders(<ReasoningBlock item={reasoning('')} />)
    expect(h.host.querySelector('.ark-reasoning__preview')).toBeNull()
  })
})