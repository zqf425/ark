// @vitest-environment jsdom
/**
 * The panel stays mounted across thread switches while every row it shows is
 * per-thread. A switch must drop the previous thread's rows immediately, a
 * superseded response must never paint, and a failed load must clear rather than
 * leave the older rows standing.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChildRunListResponse, ChildRunSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { listChildRuns } from '../api/child-runs-api'
import { ChildRunsPanel } from './ChildRunsPanel'

vi.mock('../api/child-runs-api', () => ({
  listChildRuns: vi.fn(),
  abortChildRun: vi.fn(),
  resumeChildRun: vi.fn(),
  steerChildRun: vi.fn()
}))

const listMock = vi.mocked(listChildRuns)

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'

function child(label: string): ChildRunSummary {
  return {
    id: '00000000-0000-0000-0000-0000000000c1',
    parentThreadId: THREAD_A,
    parentTurnId: '00000000-0000-0000-0000-0000000000t1',
    label,
    status: 'running',
    createdAt: '2026-10-28T00:00:00Z'
  }
}

function response(label: string): ChildRunListResponse {
  return {
    enabled: true,
    active: 1,
    childRuns: [child(label)],
    aggregates: {
      queued: 0,
      running: 1,
      completed: 0,
      failed: 0,
      aborted: 0,
      toolInvocations: 0,
      usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }
    }
  }
}

/** A promise the test resolves by hand, so response ordering is explicit. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void } {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('ChildRunsPanel thread ownership', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('never paints the previous thread rows after a switch', async () => {
    const forA = deferred<ChildRunListResponse>()
    listMock.mockReturnValueOnce(forA.promise)

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()
    forA.resolve(response('child of thread A'))
    await settle()
    expect(harness.text()).toContain('child of thread A')

    // B never answers: the rows on screen must already be gone.
    listMock.mockReturnValueOnce(new Promise<ChildRunListResponse>(() => undefined))
    await harness.render(<ChildRunsPanel threadId={THREAD_B} />)
    await settle()

    expect(harness.text()).not.toContain('child of thread A')
  })

  it('drops a late response that belongs to the previous thread', async () => {
    const forA = deferred<ChildRunListResponse>()
    listMock.mockReturnValueOnce(forA.promise)

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()

    listMock.mockReturnValueOnce(new Promise<ChildRunListResponse>(() => undefined))
    await harness.render(<ChildRunsPanel threadId={THREAD_B} />)
    await settle()

    // A's request resolves only after B became the active thread.
    forA.resolve(response('late row from thread A'))
    await settle()

    expect(harness.text()).not.toContain('late row from thread A')
  })

  it('clears the rows when the load fails', async () => {
    const forA = deferred<ChildRunListResponse>()
    listMock.mockReturnValueOnce(forA.promise)

    harness = await mountWithProviders(<ChildRunsPanel threadId={THREAD_A} />)
    await settle()
    forA.resolve(response('child of thread A'))
    await settle()
    expect(harness.text()).toContain('child of thread A')

    const failed = deferred<ChildRunListResponse>()
    listMock.mockReturnValueOnce(failed.promise)
    useCodeSessionStore.setState({ childRunRevision: useCodeSessionStore.getState().childRunRevision + 1 })
    await settle()
    failed.reject(new Error('child runs unavailable'))
    await settle()

    expect(harness.text()).not.toContain('child of thread A')
  })
})
