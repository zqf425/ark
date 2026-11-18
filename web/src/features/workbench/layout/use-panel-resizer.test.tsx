// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'
import { act, useState } from 'react'
import {
  installBrowserStubs,
  mountWithProviders,
  type ReactHarness
} from '../../../shared/testing/react-harness'
import { resizerKeyDelta, usePanelResizer } from './use-panel-resizer'

function Probe({ initialWidth, anchor }: { initialWidth: number; anchor?: 'left' | 'right' }): JSX.Element {
  const [width, setWidth] = useState(initialWidth)
  const { onPointerDown } = usePanelResizer(width, setWidth, anchor)
  return (
    <div data-testid="resizer" onPointerDown={onPointerDown}>
      {width}
    </div>
  )
}

/**
 * Count live window listeners per event type without changing behavior, so a
 * test can prove listeners are gone instead of hoping nothing else broke.
 */
function trackWindowListeners(): {
  count: (type: string) => number
  restore: () => void
} {
  const live = new Map<string, Set<EventListenerOrEventListenerObject>>()
  // Narrow the DOM overloads: the tracker only ever passes string event types
  // and nullable function listeners, which the real overloads reject.
  const originalAdd = window.addEventListener.bind(window) as (
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions
  ) => void
  const originalRemove = window.removeEventListener.bind(window) as (
    type: string,
    listener: EventListenerOrEventListenerObject | null,
    options?: boolean | EventListenerOptions
  ) => void
  const bucket = (type: string): Set<EventListenerOrEventListenerObject> => {
    let set = live.get(type)
    if (!set) {
      set = new Set()
      live.set(type, set)
    }
    return set
  }
  window.addEventListener = ((type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions) => {
    if (typeof listener === 'function') bucket(type).add(listener)
    originalAdd(type, listener, options)
  }) as unknown as typeof window.addEventListener
  window.removeEventListener = ((type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | EventListenerOptions) => {
    if (typeof listener === 'function') bucket(type).delete(listener)
    originalRemove(type, listener, options)
  }) as unknown as typeof window.removeEventListener
  return {
    count: (type: string): number => live.get(type)?.size ?? 0,
    restore: (): void => {
      window.addEventListener = originalAdd
      window.removeEventListener = originalRemove
    }
  }
}

/** jsdom cannot capture pointers; the hook only needs the calls to exist. */
function stubPointerCapture(): { release: Mock } {
  const release = vi.fn()
  const proto = HTMLElement.prototype as unknown as Record<string, unknown>
  proto.setPointerCapture = vi.fn()
  proto.releasePointerCapture = release
  return { release }
}

/**
 * jsdom has no PointerEvent constructor: dispatch a plain Event with the
 * pointer fields the hook reads attached directly.
 */
function firePointer(target: EventTarget, type: string, clientX: number): void {
  const event = new Event(type, { bubbles: true })
  Object.assign(event, { clientX, pointerId: 1 })
  target.dispatchEvent(event)
}

describe('usePanelResizer', () => {
  let harness: ReactHarness | null = null
  let track: ReturnType<typeof trackWindowListeners> | null = null
  let release: Mock

  beforeEach(() => {
    installBrowserStubs()
    release = stubPointerCapture().release
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    track?.restore()
    track = null
    vi.restoreAllMocks()
  })

  function resizer(): HTMLElement {
    const element = harness?.host.querySelector<HTMLElement>('[data-testid="resizer"]')
    if (!element) throw new Error('resizer element missing')
    return element
  }

  it('拖拽移动更新宽度并按上限截断', async () => {
    track = trackWindowListeners()
    harness = await mountWithProviders(<Probe initialWidth={300} />)
    await act(async () => firePointer(resizer(), 'pointerdown', 100))
    expect(track.count('pointermove')).toBe(1)
    expect(track.count('pointerup')).toBe(1)
    expect(track.count('pointercancel')).toBe(1)

    await act(async () => firePointer(window, 'pointermove', 200))
    expect(harness.text()).toContain('400')
    await act(async () => firePointer(window, 'pointermove', 1000))
    expect(harness.text()).toContain('640')
  })

  it('pointerup 结束拖拽：监听器移除且后续移动不再生效', async () => {
    track = trackWindowListeners()
    harness = await mountWithProviders(<Probe initialWidth={300} />)
    await act(async () => firePointer(resizer(), 'pointerdown', 100))
    await act(async () => firePointer(window, 'pointerup', 100))
    expect(track.count('pointermove')).toBe(0)
    expect(track.count('pointerup')).toBe(0)
    expect(track.count('pointercancel')).toBe(0)
    expect(release).toHaveBeenCalled()

    await act(async () => firePointer(window, 'pointermove', 500))
    expect(harness.text()).toContain('300')
  })

  it('pointercancel 同样结束拖拽', async () => {
    track = trackWindowListeners()
    harness = await mountWithProviders(<Probe initialWidth={300} />)
    await act(async () => firePointer(resizer(), 'pointerdown', 100))
    await act(async () => firePointer(window, 'pointercancel', 100))
    expect(track.count('pointermove')).toBe(0)
    expect(track.count('pointerup')).toBe(0)
    expect(track.count('pointercancel')).toBe(0)
  })

  it('拖拽中卸载组件，window 监听器随之移除（修复前会泄漏）', async () => {
    track = trackWindowListeners()
    harness = await mountWithProviders(<Probe initialWidth={240} />)
    await act(async () => firePointer(resizer(), 'pointerdown', 0))
    expect(track.count('pointermove')).toBe(1)

    await harness.unmount()
    harness = null
    expect(track.count('pointermove')).toBe(0)
    expect(track.count('pointerup')).toBe(0)
    expect(track.count('pointercancel')).toBe(0)
  })

  it('resizerKeyDelta：方向键步进 16px，其他键为 null', () => {
    expect(resizerKeyDelta('ArrowRight')).toBe(16)
    expect(resizerKeyDelta('ArrowLeft')).toBe(-16)
    expect(resizerKeyDelta('Home')).toBeNull()
  })
})

describe('usePanelResizer（右锚定：检查器左缘）', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    stubPointerCapture()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('向左拖动变宽、向右拖动变窄，并按上下限截断', async () => {
    harness = await mountWithProviders(<Probe initialWidth={392} anchor='right' />)
    const element = harness.host.querySelector<HTMLElement>('[data-testid="resizer"]')
    if (!element) throw new Error('resizer element missing')
    await act(async () => firePointer(element, 'pointerdown', 800))
    await act(async () => firePointer(window, 'pointermove', 700))
    expect(harness.text()).toContain('492')
    await act(async () => firePointer(window, 'pointermove', 900))
    expect(harness.text()).toContain('292')
    await act(async () => firePointer(window, 'pointermove', 0))
    expect(harness.text()).toContain('640')
  })
})
