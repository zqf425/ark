// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../testing/react-harness'
import { act } from 'react'
import { useThrottledValue } from './Markdown'

function Probe({ text }: { text: string }): JSX.Element {
  const value = useThrottledValue(text, 40)
  return <p id="probe">{value}</p>
}

describe('useThrottledValue', () => {
  let harness: ReactHarness | null = null

  it('静态初值立即显示，快速变更在间隔内至多落盘一次', async () => {
    installBrowserStubs()
    harness = await mountWithProviders(<Probe text="first" />)
    await settle()
    expect(harness.text()).toContain('first')

    // An immediate change is held back: no parse/render per token.
    await harness.render(<Probe text="second" />)
    expect(harness.text()).toContain('first')

    // The pending value lands after the interval.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 120))
    })
    expect(harness.text()).toContain('second')
  })

  it('变更后的最终值总会落盘', async () => {
    installBrowserStubs()
    harness = await mountWithProviders(<Probe text="a" />)
    await settle()
    for (const next of ['b', 'c', 'd', 'e']) {
      await harness.render(<Probe text={next} />)
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 60))
      })
    }
    expect(harness.text()).toContain('e')
  })
})
