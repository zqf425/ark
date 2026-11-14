// @vitest-environment jsdom
/**
 * Viewer polling lifecycle: seed from the transcript route, resume the poll
 * from `highest_seq` while active, stop when settled and keep the last good
 * content when a poll fails.
 */
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
