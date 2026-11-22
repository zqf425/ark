// @vitest-environment jsdom
/**
 * The session strip's total duration uses the same spelled-out units as the
 * per-turn clock (`耗时`/`时/分/秒`), not the old `m:ss` clock: the header total
 * and the turn totals must read alike, and the strip's total is defined as the
 * aggregate's server-folded `active_ms` (wall clock with user-gate waits
 * excluded).
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'

installBrowserStubs()

// The strip seeds its counters over HTTP; the total it prints is the
// aggregate's own `active_ms`, which is what this test pins.
vi.mock('../api/threads-api', () => ({
  getThreadUsage: () =>
    Promise.resolve({
      thread_id: '00000000-0000-0000-0000-0000000000ff',
      turns: 2,
      samples: 3,
      prompt_tokens: 0,
      completion_tokens: 0,
      total_tokens: 0,
      // 30m 45s: the session's active span across all turns.
      active_ms: 1_845_000
    })
}))

const { SessionStatsBar } = await import('./SessionStatsBar')

const T1 = '00000000-0000-0000-0000-000000000001'
const T2 = '00000000-0000-0000-0000-000000000002'

describe('SessionStatsBar total duration format', () => {
  afterEach(() => {
    useCodeSessionStore.setState({ rounds: [], usage: null, turnLastEventAt: {} })
    document.body.innerHTML = ''
  })

  it('spells the session total out in hours, minutes and seconds', async () => {
    // 10m 45s in one turn plus 20m in another = 30m 45s total. The strip reads
    // the aggregate, so the same sum arrives as one `active_ms`.
    useCodeSessionStore.setState({
      rounds: [
        { turnId: T1, durationMs: 645_000, activeMs: 645_000 },
        { turnId: T2, durationMs: 1_200_000, activeMs: 1_200_000 }
      ],
      usage: {
        samples: 3,
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
        costUsd: null,
        durationMs: 1_845_000,
        activeMs: 1_845_000,
        cacheHitTokens: null,
        cacheMissTokens: null,
        lastPromptTokens: null,
        lastRoundTokensPerSec: null
      }
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

  it('extends the total live while a turn runs, freezing on a pending gate', async () => {
    // The seed reports 10m so far; the active turn has streamed for 30 more
    // seconds, so the strip shows 10m30s without waiting for an HTTP refresh.
    const T3 = '00000000-0000-0000-0000-000000000003'
    useCodeSessionStore.setState({
      activeTurnId: T3,
      turnState: { [T3]: 'running' },
      rounds: [{ turnId: T3, durationMs: 0, activeMs: 600_000 }],
      turnLastEventAt: { [T3]: Date.now() - 30_000 },
      usage: {
        samples: 1,
        promptTokens: 0,
        completionTokens: 0,
        totalTokens: 0,
        costUsd: null,
        durationMs: 0,
        activeMs: 600_000,
        cacheHitTokens: null,
        cacheMissTokens: null,
        lastPromptTokens: null,
        lastRoundTokensPerSec: null
      }
    })

    const harness = await mountWithProviders(<SessionStatsBar threadId="00000000-0000-0000-0000-0000000000ff" />)
    await vi.waitFor(() => {
      expect(harness.host.textContent).toContain('耗时')
    })
    expect(harness.host.textContent).toContain('10分30秒')

    // A pending approval freezes the live extension at its requested_at: the
    // printed clock stays 10分30秒 even as wall time passes.
    useCodeSessionStore.setState({
      approvals: {
        a1: {
          approval_id: 'a1',
          turn_id: T3,
          tool: 'bash',
          summary: 'x',
          requested_at: new Date().toISOString(),
          status: 'pending'
        }
      }
    })
    // Let the 1s tick fire at least twice after the gate appeared.
    await new Promise((resolve) => setTimeout(resolve, 2_100))
    expect(harness.host.textContent).toContain('10分30秒')
  })
})
