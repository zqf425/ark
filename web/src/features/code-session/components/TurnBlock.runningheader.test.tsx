// @vitest-environment jsdom
/**
 * The running process card headers itself "正在执行中" (Qoder-referenced
 * layout) with the live active-span clock; a settled card keeps the
 * "执行过程 · N 步 · 耗时 …" summary line the duration contract pins.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { TurnBlock } from './TurnBlock'

installBrowserStubs()

const TURN = '00000000-0000-0000-0000-0000000000d1'

function item(id: string, kind: ThreadItem['kind'], text?: string): ThreadItem {
  return {
    item_id: id,
    turn_id: TURN,
    kind,
    status: 'completed',
    created_at: '2026-11-20T08:00:00Z',
    ...(text === undefined ? {} : { text })
  } as ThreadItem
}

function group(state: 'running' | 'completed') {
  return {
    turnId: TURN,
    state,
    items: [item('u1', 'user_message', '继续'), item('t1', 'tool_call'), item('a1', 'assistant_message', '结论')] as ThreadItem[]
  }
}

describe('TurnBlock running header', () => {
  afterEach(() => {
    useCodeSessionStore.setState({ rounds: [], turnLastEventAt: {} })
    document.body.innerHTML = ''
  })

  it('headers a running card 正在执行中, not 执行过程', async () => {
    const h = await mountWithProviders(<TurnBlock threadId="t" group={group('running')} availability="hidden" />)
    const title = h.host.querySelector('.ark-process__title')!
    expect(title.textContent).toBe('正在执行中')
  })

  it('keeps the settled summary title 执行过程', async () => {
    const h = await mountWithProviders(<TurnBlock threadId="t" group={group('completed')} availability="hidden" />)
    expect(h.host.querySelector('.ark-process__title')!.textContent).toBe('执行过程')
  })

  it('shows the live span (no 耗时 label) while running', async () => {
    useCodeSessionStore.setState({
      rounds: [{ turnId: TURN, durationMs: 61_000, activeMs: 61_000 }],
      turnLastEventAt: {}
    })
    const h = await mountWithProviders(<TurnBlock threadId="t" group={group('running')} availability="hidden" />)
    const clock = h.host.querySelector('.ark-process__duration')
    expect(clock).not.toBeNull()
    expect(clock!.textContent).toBe('1分1秒')
  })

  it('ticks when opened mid-turn before an active span seed exists', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-11-20T08:00:05Z'))
    try {
      useCodeSessionStore.setState({
        rounds: [{ turnId: TURN, durationMs: 0, activeMs: null }],
        turnLastEventAt: { [TURN]: Date.parse('2026-11-20T08:00:00Z') }
      })
      const h = await mountWithProviders(<TurnBlock threadId="t" group={group('running')} availability="hidden" />)
      expect(h.host.querySelector('.ark-process__duration')?.textContent).toBe('5秒')
      await h.unmount()
    } finally {
      vi.useRealTimers()
    }
  })
})