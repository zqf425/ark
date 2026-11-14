// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { ProviderListResponse } from '../../../shared/contracts'

vi.mock('../../settings/api/providers-api', () => ({ listProviders: vi.fn() }))
import { listProviders } from '../../settings/api/providers-api'

const listMock = vi.mocked(listProviders)

function providers(threshold: number | undefined, window?: number): ProviderListResponse {
  return {
    providers: [],
    compaction_threshold_tokens: threshold,
    context_window_tokens: window
  } as unknown as ProviderListResponse
}

describe('useContextLimit', () => {
  let harness: ReactHarness | null = null
  // The hook keeps a module-level TTL cache; a fresh import per test keeps
  // that cache from leaking a denominator between cases.
  let useContextLimit: typeof import('./use-context-limit').useContextLimit

  const mountProbe = async (): Promise<ReactHarness> => {
    const Probe = (): JSX.Element => {
      const reading = useContextLimit()
      return <p id="probe">{`${reading.window ?? 'none'}/${reading.threshold ?? 'none'}`}</p>
    }
    return mountWithProviders(<Probe />)
  }

  beforeEach(async () => {
    installBrowserStubs()
    listMock.mockReset()
    vi.resetModules()
    ;({ useContextLimit } = await import('./use-context-limit'))
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.useRealTimers()
  })

  it('窗口与阈值分别下发，TTL 到期后重取，失败时保留旧值', async () => {
    // The footer stays mounted across thread switches, so only the interval
    // guarantees a settings change is picked up at all.
    vi.useFakeTimers()
    listMock.mockResolvedValue(providers(100_000, 200_000))
    harness = await mountProbe()
    await settle()
    expect(harness.text()).toContain('200000/100000')

    listMock.mockResolvedValue(providers(200_000, 400_000))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('400000/200000')

    listMock.mockRejectedValue(new Error('providers unavailable'))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('400000/200000')
  })

  it('无 provider 时上报 null 并隐藏水位，恢复后重新给出分母', async () => {
    // A provider that declares no window now derives from the 256K default,
    // so `null` only means "no provider resolves at all".
    vi.useFakeTimers()
    listMock.mockResolvedValue(providers(undefined, undefined))
    harness = await mountProbe()
    await settle()
    expect(harness.text()).toContain('none/none')

    listMock.mockResolvedValue(providers(400_000, 500_000))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('500000/400000')
  })
})
