// @vitest-environment jsdom
/**
 * The session strip's total duration uses the same spelled-out units as the
 * per-turn clock (`耗时`/`时/分/秒`), not the old `m:ss` clock: the header total
 * and the turn totals must read alike, and the strip's total is defined as the
 * sum of those same per-round measurements.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'

installBrowserStubs()

// The strip seeds its counters over HTTP; the rounds it sums come from the
// store, which is all this test pins.
vi.mock('../api/threads-api', () => ({
  getThreadUsage: () => Promise.resolve({ turns: 2, samples: 3, prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 })
}))
vi.mock('../api/llm-rounds-api', () => ({
  listLlmRounds: () => Promise.resolve({ rounds: [{}, {}, {}], truncated: false })
}))

const { SessionStatsBar } = await import('./SessionStatsBar')

const T1 = '00000000-0000-0000-0000-000000000001'
const T2 = '00000000-0000-0000-0000-000000000002'

describe('SessionStatsBar total duration format', () => {
  afterEach(() => {
    useCodeSessionStore.setState({ rounds: [], usage: null })
    document.body.innerHTML = ''
  })

  it('spells the session total out in hours, minutes and seconds', async () => {
    // 10m 45s in one turn plus 20m in another = 30m 45s total.
    useCodeSessionStore.setState({
      rounds: [
        { turnId: T1, durationMs: 645_000 },
        { turnId: T2, durationMs: 1_200_000 }
      ]
    })

    const harness = await mountWithProviders(<SessionStatsBar threadId="00000000-0000-0000-0000-0000000000ff" />)
    // Let the HTTP seed settle so the rounds segment renders.
    await vi.waitFor(() => {
      expect(harness.host.textContent).toContain('耗时')
    })

    const text = harness.host.textContent ?? ''
    expect(text).toContain('耗时 30分45秒')
    // Not the old `m:ss` clock.
    expect(text).not.toContain('30:45')
  })
})
