// @vitest-environment jsdom
/**
 * The expanded row: the viewer seeds from the transcript route and keeps
 * polling while the run is active; the steer box sends through the API and the
 * panel re-reads the list. Ownership rules of the collapsed panel are covered
 * by the older suites; here the expansion behavior is the unit under test.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChildRunListResponse, ChildRunSummary, ThreadDetail } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { abortChildRun, getChildRunEvents, getChildRunThread, listChildRuns, steerChildRun } from '../api/child-runs-api'
import { ChildRunsPanel } from './ChildRunsPanel'

vi.mock('../api/child-runs-api', () => ({
  listChildRuns: vi.fn(),
  abortChildRun: vi.fn(),
  resumeChildRun: vi.fn(),
  steerChildRun: vi.fn(),
  getChildRunThread: vi.fn(),
  getChildRunEvents: vi.fn()
}))

const listMock = vi.mocked(listChildRuns)
const steerMock = vi.mocked(steerChildRun)
const abortMock = vi.mocked(abortChildRun)
const detailMock = vi.mocked(getChildRunThread)
const eventsMock = vi.mocked(getChildRunEvents)

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const CHILD = '00000000-0000-0000-0000-0000000000c1'
const TURN = '00000000-0000-0000-0000-0000000000t1'

function child(overrides: Partial<ChildRunSummary>): ChildRunSummary {
  return {
    id: CHILD,
    parent_thread_id: THREAD_A,
    parent_turn_id: TURN,
    label: 'research task',
    status: 'running',
    created_at: '2026-10-28T00:00:00Z',
    ...overrides
  }
}

function response(runs: ChildRunSummary[]): ChildRunListResponse {
  return {
    enabled: true,
    active: runs.filter((run) => run.status === 'running' || run.status === 'queued').length,
    child_runs: runs,
    aggregates: {
      queued: 0,
      running: 1,
      completed: 0,
      failed: 0,
      aborted: 0,
      tool_invocations: 0,
      usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }
    }
  }
}

function threadDetail(lastSeq: number): ThreadDetail {
  return {
    summary: {
      thread_id: CHILD,
      title: 'research task',
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

/** Simulate a controlled-input keystroke the way jsdom can: set + input event. */
function typeInto(textarea: HTMLTextAreaElement, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!
  setter.call(textarea, value)
  textarea.dispatchEvent(new Event('input', { bubbles: true }))
}

describe('ChildRunsPanel expansion and steer', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    steerMock.mockReset()
    abortMock.mockReset()
    detailMock.mockReset()
    eventsMock.mockReset()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.useRealTimers()
  })

  it('expands a row, seeds the viewer from /thread and stops polling when settled', async () => {
    listMock.mockResolvedValue(response([child({ status: 'completed' })]))
    detailMock.mockResolvedValue(threadDetail(12))
    // A settled run: no poll scheduled after the first one.
    eventsMock.mockResolvedValue({ events: [], highest_seq: 12 })

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()
    expect(harness.text()).toContain('research task')

    const button = harness.host.querySelector<HTMLButtonElement>('.ark-childcard__head')
    expect(button).not.toBeNull()
    await harness.render(<ChildRunsPanel threadId={THREAD_A} />)
    button!.click()
    await settle()

    expect(detailMock).toHaveBeenCalledWith(THREAD_A, CHILD, expect.anything())
    expect(harness.text()).not.toContain('实时跟踪中')
  })

  it('sends the steer box text through the API and clears the draft', async () => {
    listMock.mockResolvedValue(response([child({})]))
    steerMock.mockResolvedValue({ id: CHILD, ok: true })
    // The post-action refresh re-reads the list.
    listMock.mockResolvedValueOnce(response([child({})]))
    detailMock.mockResolvedValue(threadDetail(3))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 3 })

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()
    const button = harness.host.querySelector<HTMLButtonElement>('.ark-childcard__head')!
    button.click()
    await settle()

    const textarea = harness.host.querySelector<HTMLTextAreaElement>('.ark-childcard__steerInput')
    expect(textarea).not.toBeNull()
    typeInto(textarea!, '先跑测试再总结')
    await settle()

    const send = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-childcard__steer .ark-childruns__actions button')][0]
    expect(send).toBeDefined()
    send!.click()
    await settle()

    expect(steerMock).toHaveBeenCalledWith(THREAD_A, CHILD, '先跑测试再总结')
  })

  it('abort from the expanded viewer calls the abort API', async () => {
    abortMock.mockResolvedValue({ id: CHILD, status: 'aborted' })
    // First load (mount) reports the run as active; the post-abort refresh
    // observes the terminal state.
    listMock.mockResolvedValue(response([child({ status: 'aborted' })]))
    listMock.mockResolvedValueOnce(response([child({})]))
    detailMock.mockResolvedValue(threadDetail(3))
    eventsMock.mockResolvedValue({ events: [], highest_seq: 3 })

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()
    const button = harness.host.querySelector<HTMLButtonElement>('.ark-childcard__head')!
    button.click()
    await settle()

    const buttons = [...harness.host.querySelectorAll<HTMLButtonElement>('.ark-childcard__steer .ark-childruns__actions button')]
    const abort = buttons.find((node) => node.textContent === '终止')
    expect(abort).toBeDefined()
    abort!.click()
    await settle()

    expect(abortMock).toHaveBeenCalledWith(THREAD_A, CHILD)
  })
})
