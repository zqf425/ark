// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { ProviderListResponse } from '../../../shared/contracts'

vi.mock('../../settings/api/providers-api', () => ({ listProviders: vi.fn() }))
import { listProviders } from '../../settings/api/providers-api'

const listMock = vi.mocked(listProviders)

function providers(threshold: number | undefined): ProviderListResponse {
  return { providers: [], compaction_threshold_tokens: threshold } as unknown as ProviderListResponse
}

describe('useContextLimit', () => {
  let harness: ReactHarness | null = null
  // The hook keeps a module-level TTL cache; a fresh import per test keeps
  // that cache from leaking a denominator between cases.
  let useContextLimit: typeof import('./use-context-limit').useContextLimit

  const mountProbe = async (): Promise<ReactHarness> => {
    const Probe = (): JSX.Element => {
      const limit = useContextLimit()
      return <p id="probe">{limit ?? 'none'}</p>
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

  it('TTL 到期后重取并更新分母，失败时保留旧值', async () => {
    // The footer stays mounted across thread switches, so only the interval
    // guarantees a settings change is picked up at all.
    vi.useFakeTimers()
    listMock.mockResolvedValue(providers(100_000))
    harness = await mountProbe()
    await settle()
    expect(harness.text()).toContain('100000')

    listMock.mockResolvedValue(providers(200_000))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('200000')

    listMock.mockRejectedValue(new Error('providers unavailable'))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('200000')
  })

  it('未声明窗口时上报 null，恢复后重新给出分母', async () => {
    vi.useFakeTimers()
    listMock.mockResolvedValue(providers(undefined))
    harness = await mountProbe()
    await settle()
    expect(harness.text()).toContain('none')

    listMock.mockResolvedValue(providers(400_000))
    await vi.advanceTimersByTimeAsync(60_000)
    await settle()
    expect(harness.text()).toContain('400000')
  })
})
