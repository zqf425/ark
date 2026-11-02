// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { RuntimeConfigView } from '../../../shared/contracts'
import { useContextLimit } from './use-context-limit'

vi.mock('../../settings/api/settings-api', () => ({ getRuntimeConfig: vi.fn() }))
import { getRuntimeConfig } from '../../settings/api/settings-api'

const configMock = vi.mocked(getRuntimeConfig)

function config(limit: number): RuntimeConfigView {
  return { tuning: { context_limit_tokens: limit } } as unknown as RuntimeConfigView
}

function Probe(): JSX.Element {
  const limit = useContextLimit()
  return <p id="probe">{limit ?? 'none'}</p>
}

describe('useContextLimit', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    configMock.mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.useRealTimers()
  })

  it('TTL 到期后重取并更新分母，失败时保留旧值', async () => {
    // The footer stays mounted across thread switches, so only the interval
    // guarantees a settings change is picked up at all.
    vi.useFakeTimers()
    configMock.mockResolvedValue(config(100_000))
    harness = await mountWithProviders(<Probe />)
    await settle()
    expect(harness.text()).toContain('100000')

    configMock.mockResolvedValue(config(200_000))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('200000')

    configMock.mockRejectedValue(new Error('config unavailable'))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('200000')
  })
})
