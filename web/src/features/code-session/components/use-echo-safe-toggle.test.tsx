// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, describe, expect, it } from 'vitest'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { useEchoSafeToggle } from './use-echo-safe-toggle'

installBrowserStubs()

let hook: { open: boolean; onToggle: (event: { currentTarget: { open: boolean } }) => void }

function Probe({ live }: { live: boolean }): null {
  hook = useEchoSafeToggle(live)
  return null
}

async function macrotask(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

describe('useEchoSafeToggle', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('follows live while unpinned', async () => {
    const harness = await mountWithProviders(<Probe live />)
    expect(hook.open).toBe(true)
    await harness.render(<Probe live={false} />)
    expect(hook.open).toBe(false)
  })

  it('ignores a toggle echoing the rendered open value', async () => {
    const harness = await mountWithProviders(<Probe live />)
    expect(hook.open).toBe(true)
    // Mount echo: reports open=true, which is exactly what React rendered.
    act(() => {
      hook.onToggle({ currentTarget: { open: true } })
    })
    await harness.render(<Probe live={false} />)
    expect(hook.open).toBe(false)
  })

  it('pins a toggle that disagrees with the rendered open value', async () => {
    const harness = await mountWithProviders(<Probe live />)
    act(() => {
      hook.onToggle({ currentTarget: { open: false } })
    })
    expect(hook.open).toBe(false)
    // Still live: the pin keeps it closed.
    await harness.render(<Probe live />)
    await macrotask()
    expect(hook.open).toBe(false)
  })

  it('updates the echo baseline after each render, not only on mount', async () => {
    const harness = await mountWithProviders(<Probe live />)
    act(() => {
      hook.onToggle({ currentTarget: { open: false } }) // user closes → pinned false
    })
    await harness.render(<Probe live />)
    expect(hook.open).toBe(false)
    // Now an echo of React's own write (open=false) must NOT pin anything new.
    act(() => {
      hook.onToggle({ currentTarget: { open: false } })
    })
    // User reopens (true ≠ false) → pinned true.
    act(() => {
      hook.onToggle({ currentTarget: { open: true } })
    })
    expect(hook.open).toBe(true)
    // Still the same phase: the pin holds.
    await harness.render(<Probe live />)
    await macrotask()
    expect(hook.open).toBe(true)
  })

  it('drops the user pin when the turn settles: a settled card folds back', async () => {
    const harness = await mountWithProviders(<Probe live />)
    // Close and reopen mid-turn: the pin now says "open" while the turn runs.
    act(() => {
      hook.onToggle({ currentTarget: { open: false } })
    })
    act(() => {
      hook.onToggle({ currentTarget: { open: true } })
    })
    expect(hook.open).toBe(true)

    // The turn settles: the pin belonged to the live phase only, so the card
    // must collapse to the settled default instead of staying open.
    await harness.render(<Probe live={false} />)
    await macrotask()
    expect(hook.open).toBe(false)
  })

  it('keeps a settled-phase pin: reopening a finished card stays open', async () => {
    const harness = await mountWithProviders(<Probe live={false} />)
    expect(hook.open).toBe(false)
    act(() => {
      hook.onToggle({ currentTarget: { open: true } })
    })
    await harness.render(<Probe live={false} />)
    await macrotask()
    expect(hook.open).toBe(true)
  })
})
