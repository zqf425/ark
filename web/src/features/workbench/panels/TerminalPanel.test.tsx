// @vitest-environment jsdom
/**
 * The panel renders one thread's completed shell commands while staying mounted
 * across thread switches. The two paths this test pins are the ones that used to
 * keep the previous thread's entries alive: the switch commit itself, and a
 * response that arrives after the switch.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { TerminalEntriesResponse } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getTerminalEntries } from '../api/workbench-api'
import { TerminalPanel } from './TerminalPanel'

vi.mock('../api/workbench-api', () => ({
  getChanges: vi.fn(),
  getFilePreview: vi.fn(),
  getPlan: vi.fn(),
  getTerminalEntries: vi.fn(),
  updateGoal: vi.fn(),
  clearGoal: vi.fn(),
  updateTodos: vi.fn()
}))

const entriesMock = vi.mocked(getTerminalEntries)

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'

/** A promise the test settles by hand, so response ordering is explicit. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void } {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function entries(command: string): TerminalEntriesResponse {
  return {
    entries: [
      {
        call_id: '00000000-0000-0000-0000-0000000000k1',
        turn_id: '00000000-0000-0000-0000-0000000000t1',
        command,
        output: '',
        ok: true,
        duration_ms: 1,
        seq: 1
      }
    ]
  }
}

function pending<T>(): Promise<T> {
  return new Promise<T>(() => undefined)
}

describe('TerminalPanel thread ownership', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    entriesMock.mockReset()
    useCodeSessionStore.getState().resetItems()
    useCodeSessionStore.setState({ activeThreadId: null })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('clears the previous thread entries as soon as the thread changes', async () => {
    const forA = deferred<TerminalEntriesResponse>()
    entriesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<TerminalPanel />)
    await settle()
    forA.resolve(entries('ls-from-thread-a'))
    await settle()
    expect(harness.text()).toContain('ls-from-thread-a')

    // B never answers: A's entries must already be gone from the switch commit.
    entriesMock.mockReturnValueOnce(pending<TerminalEntriesResponse>())
    useCodeSessionStore.setState({ activeThreadId: THREAD_B })
    await settle()

    expect(harness.text()).not.toContain('ls-from-thread-a')
  })

  it('never paints a response that belongs to the previous thread', async () => {
    const forA = deferred<TerminalEntriesResponse>()
    entriesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<TerminalPanel />)
    await settle()

    entriesMock.mockReturnValueOnce(pending<TerminalEntriesResponse>())
    useCodeSessionStore.setState({ activeThreadId: THREAD_B })
    await settle()

    // A resolves only after B became the active thread.
    forA.resolve(entries('late-ls-from-thread-a'))
    await settle()

    expect(harness.text()).not.toContain('late-ls-from-thread-a')
  })
})
