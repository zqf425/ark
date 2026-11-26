// @vitest-environment jsdom
/**
 * The kebab menu on a rail row must stay reachable for the bottom rows of a
 * viewport-bound list: the delete item is the menu's last entry, and a
 * menu that always opens *below* its kebab falls outside the window for
 * those rows. The menu must flip above when the measured height runs out of
 * room below (and clamp its left edge to the viewport).
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { ThreadListPanel } from './ThreadListPanel'

vi.mock('../api/threads-api', () => ({
  listThreads: vi.fn(),
  createThread: vi.fn(),
  deleteThread: vi.fn(),
  deleteThreadsByWorkspace: vi.fn(),
  forkThread: vi.fn(),
  updateThread: vi.fn(),
  exportThread: vi.fn()
}))

vi.mock('../hooks/use-thread-running', () => ({
  useThreadRunning: () => undefined
}))

vi.mock('../../settings/api/workspace-api', () => ({
  getWorkspaceView: vi.fn().mockResolvedValue({ active: null, roots: [] })
}))

vi.mock('./ScheduleCreateDialog', () => ({
  ScheduleCreateDialog: () => null
}))

function thread(id: string, title: string): ThreadSummary {
  return {
    thread_id: id,
    title,
    created_at: '2026-10-28T00:00:00Z',
    updated_at: '2026-10-28T00:00:00Z',
    archived: false,
    running: false
  }
}

function installViewport(width: number, height: number, kebabBottom = 72): void {
  Object.defineProperty(window, 'innerWidth', { value: width, configurable: true })
  Object.defineProperty(window, 'innerHeight', { value: height, configurable: true })
  Element.prototype.getBoundingClientRect = function (): DOMRect {
    const bottomRow = Boolean(
      (this as HTMLElement).className?.toString().includes('kebab') &&
        (this as HTMLElement).closest('[data-bottom-row]')
    )
    if (bottomRow) return makeRect(window.innerHeight - 36, window.innerHeight - 4)
    if ((this as HTMLElement).classList?.contains('ark-threadrow__menu')) return makeRect(0, 220)
    return makeRect(kebabBottom - 32, kebabBottom)
  }
}

function makeRect(top: number, bottom: number): DOMRect {
  return {
    x: 200, y: top, top, bottom, left: 200, right: 236,
    width: 36, height: bottom - top, toJSON: () => ({})
  } as DOMRect
}

describe('ThreadRow kebab menu placement', () => {
  let harness: ReactHarness | null = null
  const realRect = Element.prototype.getBoundingClientRect

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.getState().setThreads([thread('00000000-0000-0000-0000-00000000000a', '最下面的会话')])
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    Element.prototype.getBoundingClientRect = realRect
  })

  it('moves focus with menu keys, closes on Tab, and restores the kebab on Escape', async () => {
    installViewport(1024, 700)
    harness = await mountWithProviders(<ThreadListPanel />)
    const trigger = harness.host.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => { trigger.click() })
    const items = [...document.querySelectorAll<HTMLButtonElement>('.ark-threadrow__menu [role="menuitem"]')]
    expect(document.activeElement).toBe(items[0])
    await act(async () => { items[0]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true })) })
    expect(document.activeElement).toBe(items[items.length - 1])
    await act(async () => { items.at(-1)!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })) })
    expect(document.activeElement).toBe(items[0])
    await act(async () => { items[0]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true })) })
    expect(document.activeElement).toBe(items.at(-1))
    const tab = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true })
    await act(async () => { items.at(-1)!.dispatchEvent(tab) })
    expect(tab.defaultPrevented).toBe(false)
    expect(document.querySelector('.ark-threadrow__menu')).toBeNull()
    await act(async () => { trigger.click() })
    await act(async () => { document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })) })
    expect(document.querySelector('.ark-threadrow__menu')).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it('flips the bottom row menu above the kebab so the delete item stays reachable', async () => {
    installViewport(1024, 700)
    harness = await mountWithProviders(<ThreadListPanel />)
    const rows = harness.host.querySelectorAll<HTMLElement>('.ark-threadrow')
    expect(rows.length).toBe(1)
    rows[0]!.setAttribute('data-bottom-row', '')
    const kebab = rows[0]!.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => { kebab.click() })
    const menu = document.querySelector<HTMLElement>('.ark-threadrow__menu')
    expect(menu).not.toBeNull()
    expect(Number.isNaN(parseFloat(menu!.style.top))).toBe(true)
    const bottom = parseFloat(menu!.style.bottom)
    // Anchored to the kebab's top edge with a 4px gap: measured against the
    // real menu height, not a hard-coded 184px estimate.
    expect(Number.isNaN(bottom)).toBe(false)
    expect(bottom).toBeLessThanOrEqual(window.innerHeight - 4)
  })

  it('flips upwards when the remaining space cannot fit the measured menu', async () => {
    installViewport(1024, 240, 236)
    harness = await mountWithProviders(<ThreadListPanel />)
    const kebab = harness.host.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => { kebab.click() })
    const menu = document.querySelector<HTMLElement>('.ark-threadrow__menu')
    expect(menu).not.toBeNull()
    expect(Number.isNaN(parseFloat(menu!.style.top))).toBe(true)
    expect(Number.isNaN(parseFloat(menu!.style.bottom))).toBe(false)
  })

  it('keeps opening downwards for rows with room below', async () => {
    installViewport(1024, 700)
    harness = await mountWithProviders(<ThreadListPanel />)
    const rows = harness.host.querySelectorAll<HTMLElement>('.ark-threadrow')
    const kebab = rows[0]!.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => { kebab.click() })
    const menu = document.querySelector<HTMLElement>('.ark-threadrow__menu')
    expect(menu).not.toBeNull()
    const top = parseFloat(menu!.style.top)
    expect(Number.isNaN(top)).toBe(false)
    expect(Number.isNaN(parseFloat(menu!.style.bottom))).toBe(true)
    expect(top).toBeCloseTo(76, 0)
  })
})
