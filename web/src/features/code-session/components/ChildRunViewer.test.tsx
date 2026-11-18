// @vitest-environment jsdom
/**
 * Viewer polling lifecycle: seed from the transcript route, resume the poll
 * from `highest_seq` while active, stop when settled and keep the last good
 * content when a poll fails.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadDetail } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { getChildRunEvents, getChildRunThread } from '../api/child-runs-api'
import { ChildRunViewer } from './ChildRunViewer'

vi.mock('../api/child-runs-api', () => ({
  getChildRunThread: vi.fn(),
  getChildRunEvents: vi.fn()
}))

vi.mock('../../../shared/api/error-describe', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../shared/api/error-describe')>()),
  // Keep the message stable without pulling the i18n catalog into this suite.
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error))
}))

const detailMock = vi.mocked(getChildRunThread)
const eventsMock = vi.mocked(getChildRunEvents)

const THREAD = '00000000-0000-0000-0000-00000000000a'
const CHILD = '00000000-0000-0000-0000-0000000000c1'

function threadDetail(lastSeq: number): ThreadDetail {
  return {
    summary: {
      thread_id: CHILD,
      title: 'child',
      created_at: '2026-10-28T00:00:00Z',
      updated_at: '2026-10-28T00:00:00Z',
      last_seq: lastSeq,
      running: true,
      archived: false
    },
    items: [],
    child_ids: [],
    child_completions: [],
    attachment_ids: [],
    runtime: {},
    runtime_snapshot: { pending_approvals: [], pending_user_inputs: [] }
  }
}

describe('ChildRunViewer polling', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    detailMock.mockReset()
    eventsMock.mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('polls again from highest_seq while the run is active', async () => {
    vi.useFakeTimers()
    detailMock.mockResolvedValue(threadDetail(2))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 2 })
    harness = await mountWithProviders(<ChildRunViewer threadId={THREAD} childId={CHILD} active />)
    await settle()
    expect(eventsMock).toHaveBeenCalledTimes(1)
    expect(eventsMock).toHaveBeenLastCalledWith(THREAD, CHILD, 2, expect.anything())
    // The 1.5s timer schedules the next poll; advancing it must fire exactly one.
    await vi.advanceTimersByTimeAsync(1600)
    expect(eventsMock).toHaveBeenCalledTimes(2)
    expect(eventsMock).toHaveBeenLastCalledWith(THREAD, CHILD, 2, expect.anything())
  })

  it('does not schedule a poll when the run is settled', async () => {
    vi.useFakeTimers()
    detailMock.mockResolvedValue(threadDetail(5))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 5 })
    harness = await mountWithProviders(<ChildRunViewer threadId={THREAD} childId={CHILD} active={false} />)
    await settle()
    expect(eventsMock).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(5000)
    expect(eventsMock).toHaveBeenCalledTimes(1)
  })

  it('keeps the last good transcript when a poll fails', async () => {
    vi.useFakeTimers()
    detailMock.mockResolvedValue(threadDetail(7))
    eventsMock.mockResolvedValueOnce({ events: [], highest_seq: 7 })
    eventsMock.mockRejectedValue(new Error('boom'))
    harness = await mountWithProviders(<ChildRunViewer threadId={THREAD} childId={CHILD} active />)
    await settle()
    expect(harness.text()).not.toContain('boom')
    await vi.advanceTimersByTimeAsync(1600)
    await settle()
    // The stale hint names the error; the empty-state line stays replaced.
    expect(harness.text()).toContain('boom')
  })
})

/**
 * The transcript fold: the child viewer renders through the same fold policy
 * as the parent process card (only the newest `RECENT_ACTIVITY_ROW_LIMIT`
 * activity rows visible, earlier ones behind the same summary row), so a long
 * child run scans by its latest activity instead of a wall of cards.
 */
describe('ChildRunViewer transcript fold', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    detailMock.mockReset()
    eventsMock.mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.useRealTimers()
  })

  function detailWithItems(itemCount: number, lastSeq: number): ThreadDetail {
    const items = Array.from({ length: itemCount }, (_, index) => ({
      item_id: `00000000-0000-0000-0000-0000000000i${index}`,
      turn_id: CHILD,
      kind: 'tool_call' as const,
      status: 'completed' as const,
      created_at: '2026-10-28T00:00:00Z'
    }))
    return { ...threadDetail(lastSeq), items }
  }

  async function mountViewer(active: boolean): Promise<ReactHarness> {
    const mounted = await mountWithProviders(
      <ChildRunViewer threadId={THREAD} childId={CHILD} active={active} />
    )
    await settle()
    return mounted
  }

  it('keeps only the newest activity rows visible and folds the rest', async () => {
    vi.useFakeTimers()
    detailMock.mockResolvedValue(detailWithItems(5, 5))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 5 })
    harness = await mountViewer(false)

    // Five tool calls, three visible: the two earliest fold behind the summary.
    const summary = harness.host.querySelector('.ark-foldedtools')
    expect(summary).not.toBeNull()
    expect(summary!.textContent).toContain('2')
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(3)
  })

  it('expands the folded rows on click and folds them back on a second click', async () => {
    // Real timers: the click assertion needs a genuine macrotask tick for the
    // re-render, and this suite never schedules poll timers (active=false).
    detailMock.mockResolvedValue(detailWithItems(5, 5))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 5 })
    harness = await mountViewer(false)

    await act(async () => {
      ;(harness!.host.querySelector('.ark-foldedtools') as HTMLElement).click()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(5)

    await act(async () => {
      ;(harness!.host.querySelector('.ark-foldedtools') as HTMLElement).click()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(3)
  })

  it('does not fold a transcript with three or fewer activity rows', async () => {
    detailMock.mockResolvedValue(detailWithItems(3, 3))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 3 })
    harness = await mountViewer(false)

    expect(harness.host.querySelector('.ark-foldedtools')).toBeNull()
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(3)
  })

  it('renders a folded error row in place while the activity around it folds', async () => {
    const items = [
      { item_id: '00000000-0000-0000-0000-0000000000i0', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:00Z' },
      { item_id: '00000000-0000-0000-0000-0000000000e1', turn_id: CHILD, kind: 'error' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:01Z', text: 'early failure' },
      { item_id: '00000000-0000-0000-0000-0000000000i2', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:02Z' },
      { item_id: '00000000-0000-0000-0000-0000000000i3', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:03Z' },
      { item_id: '00000000-0000-0000-0000-0000000000i4', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:04Z' }
    ]
    detailMock.mockResolvedValue({ ...threadDetail(9), items })
    eventsMock.mockResolvedValue({ events: [], highest_seq: 9 })
    harness = await mountViewer(false)

    // Four activity rows fold; the error between them never folds away.
    expect(harness.host.querySelector('.ark-foldedtools')).not.toBeNull()
    expect(harness.host.textContent).toContain('early failure')
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(3)
  })

  it('keeps a user input row visible while the activity around it folds', async () => {
    // The child run's transcript carries the delegation text and steered
    // instructions as user_message rows; they are the human side of the
    // conversation and must stay visible even when they sit inside the folded
    // earlier slice - only model activity folds.
    const items = [
      { item_id: '00000000-0000-0000-0000-0000000000i0', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:00Z' },
      { item_id: '00000000-0000-0000-0000-0000000000u1', turn_id: CHILD, kind: 'user_message' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:01Z', text: '先跑测试再改' },
      { item_id: '00000000-0000-0000-0000-0000000000i2', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:02Z' },
      { item_id: '00000000-0000-0000-0000-0000000000i3', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:03Z' },
      { item_id: '00000000-0000-0000-0000-0000000000i4', turn_id: CHILD, kind: 'tool_call' as const, status: 'completed' as const, created_at: '2026-10-28T00:00:04Z' }
    ]
    detailMock.mockResolvedValue({ ...threadDetail(9), items })
    eventsMock.mockResolvedValue({ events: [], highest_seq: 9 })
    harness = await mountViewer(false)

    expect(harness.host.querySelector('.ark-foldedtools')).not.toBeNull()
    // The steered user input inside the folded slice stays on screen.
    expect(harness.host.textContent).toContain('先跑测试再改')
    expect(harness.host.querySelectorAll('.ark-tool')).toHaveLength(3)
  })
})
