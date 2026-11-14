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
    // User opened it while already open? No: user CLOSED it (false ≠ true).
    act(() => {
      hook.onToggle({ currentTarget: { open: false } })
    })
    expect(hook.open).toBe(false)
    // live flips back true — the pin wins.
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
    // Pin holds even when live goes false.
    await harness.render(<Probe live={false} />)
    await macrotask()
    expect(hook.open).toBe(true)
  })
})
