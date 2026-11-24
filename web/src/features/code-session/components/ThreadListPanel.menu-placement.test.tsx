// @vitest-environment jsdom
/**
 * The kebab menu on a rail row must stay reachable for the bottom rows of a
 * viewport-bound list: the delete item is the menu's last entry, and a
 * menu that always opens *below* its kebab falls outside the window for
 * those rows — the user literally cannot click "删除". The open logic has
 * to flip the menu above the button (and clamp the right edge) when the
 * space below runs out.
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
  // No timers, no polling in this suite.
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

/** Install a deterministic viewport the component can measure against. */
function installViewport(width: number, height: number, kebabBottom = 72): void {
  Object.defineProperty(window, 'innerWidth', { value: width, configurable: true })
  Object.defineProperty(window, 'innerHeight', { value: height, configurable: true })
  Element.prototype.getBoundingClientRect = function (): DOMRect {
    // Every kebab of the bottom thread row sits flush with the viewport
    // bottom edge; rows above stay comfortably inside.
    const bottomRow = Boolean(
      (this as HTMLElement).className?.toString().includes('kebab') &&
        (this as HTMLElement).closest('[data-bottom-row]')
    )
    if (bottomRow) {
      return makeRect(window.innerHeight - 36, window.innerHeight - 4)
    }
    return makeRect(kebabBottom - 32, kebabBottom)
  }
}

function makeRect(top: number, bottom: number): DOMRect {
  return {
    x: 200,
    y: top,
    top,
    bottom,
    left: 200,
    right: 236,
    width: 36,
    height: bottom - top,
    toJSON: () => ({})
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

  it('flips the bottom row menu above the kebab so the delete item stays reachable', async () => {
    installViewport(1024, 700)
    harness = await mountWithProviders(<ThreadListPanel />)
    // Mark the row as the last one before the viewport stub can see it.
    const rows = harness.host.querySelectorAll<HTMLElement>('.ark-threadrow')
    expect(rows.length).toBe(1)
    rows[0]!.setAttribute('data-bottom-row', '')

    const kebab = rows[0]!.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => {
      kebab.click()
    })

    const menu = document.querySelector<HTMLElement>('.ark-threadrow__menu')
    expect(menu).not.toBeNull()
    // Five items (rename, fork, archive, export, delete) ≈ 5 * 32px + padding.
    const top = parseFloat(menu!.style.top)
    const bottom = parseFloat(menu!.style.bottom)
    // Bottom-anchored: no `top`, the menu grows upwards from the kebab's top
    // edge minus the same 4px gap the downward case uses:
    // viewportFloor - bottom === rect.top - 4.
    expect(Number.isNaN(top)).toBe(true)
    const viewportFloor = window.innerHeight - 8
    expect(viewportFloor - bottom).toBeCloseTo(window.innerHeight - 36 - 4, 0)
  })

  it('flips upwards when the remaining space only fits the old menu estimate', async () => {
    installViewport(1024, 700, 508)
    harness = await mountWithProviders(<ThreadListPanel />)
    const kebab = harness.host.querySelector<HTMLButtonElement>('.ark-threadrow__kebab')!
    await act(async () => {
      kebab.click()
    })

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
    await act(async () => {
      kebab.click()
    })

    const menu = document.querySelector<HTMLElement>('.ark-threadrow__menu')
    expect(menu).not.toBeNull()
    const top = parseFloat(menu!.style.top)
    const bottom = parseFloat(menu!.style.bottom)
    expect(Number.isNaN(top)).toBe(false)
    expect(Number.isNaN(bottom)).toBe(true)
    // rect.bottom + 4 for a mid-list row.
    expect(top).toBeCloseTo(76, 0)
  })
})
