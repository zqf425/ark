// @vitest-environment jsdom
/**
 * The panel renders one thread's file changes while staying mounted across
 * thread switches. Every path that used to keep the previous thread's rows
 * alive is pinned here: the switch commit itself, a response that arrives after
 * the switch, a load that fails, and a cleared selection.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import type { WorkspaceChangesResponse, WorkspaceDiff, WorkspaceView } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getWorkspaceDiff, getWorkspaceView } from '../../settings/api/workspace-api'
import { getChanges } from '../api/workbench-api'
import { ChangesPanel } from './ChangesPanel'

vi.mock('../api/workbench-api', () => ({
  getChanges: vi.fn(),
  getFilePreview: vi.fn(),
  getPlan: vi.fn(),
  getTerminalEntries: vi.fn(),
  updateGoal: vi.fn(),
  clearGoal: vi.fn(),
  updateTodos: vi.fn()
}))

vi.mock('../../settings/api/workspace-api', () => ({
  getWorkspaceView: vi.fn(),
  getWorkspaceDiff: vi.fn(),
  getWorkspaceStatus: vi.fn(),
  switchWorkspace: vi.fn(),
  validateWorkspace: vi.fn()
}))

const changesMock = vi.mocked(getChanges)
const viewMock = vi.mocked(getWorkspaceView)
const diffMock = vi.mocked(getWorkspaceDiff)

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

function changes(path: string): WorkspaceChangesResponse {
  return { changes: [{ path, operation: 'write', turn_id: '00000000-0000-0000-0000-0000000000t1', seq: 1 }] }
}

const workspaceView: WorkspaceView = { recent: [], generation: 1 }
const workspaceDiff: WorkspaceDiff = {
  generation: 1,
  staged: [],
  unstaged: [],
  untracked: [],
  file_count: 0,
  patch_bytes: 0,
  truncated: false
}

function pending<T>(): Promise<T> {
  return new Promise<T>(() => undefined)
}

describe('ChangesPanel thread ownership', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    changesMock.mockReset()
    viewMock.mockReset()
    diffMock.mockReset()
    viewMock.mockResolvedValue(workspaceView)
    diffMock.mockResolvedValue(workspaceDiff)
    useCodeSessionStore.getState().resetItems()
    useCodeSessionStore.setState({ activeThreadId: null })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('clears the previous thread rows as soon as the thread changes', async () => {
    const forA = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()
    forA.resolve(changes('from-thread-a.ts'))
    await settle()
    expect(harness.text()).toContain('from-thread-a.ts')

    // B never answers: A's rows must already be gone from the switch commit.
    changesMock.mockReturnValueOnce(pending<WorkspaceChangesResponse>())
    useCodeSessionStore.setState({ activeThreadId: THREAD_B })
    await settle()

    expect(harness.text()).not.toContain('from-thread-a.ts')
  })

  it('never paints a response that belongs to the previous thread', async () => {
    const forA = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()

    changesMock.mockReturnValueOnce(pending<WorkspaceChangesResponse>())
    useCodeSessionStore.setState({ activeThreadId: THREAD_B })
    await settle()

    // A resolves only after B became the active thread.
    forA.resolve(changes('late-row-from-a.ts'))
    await settle()

    expect(harness.text()).not.toContain('late-row-from-a.ts')
  })

  it('never paints a superseded reload of the same thread', async () => {
    const first = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(first.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()

    // A reload starts while the first request is still in flight.
    const second = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(second.promise)
    useCodeSessionStore.setState({ lastSeq: useCodeSessionStore.getState().lastSeq + 1 })
    await settle()
    second.resolve(changes('newer-row.ts'))
    await settle()
    expect(harness.text()).toContain('newer-row.ts')

    first.resolve(changes('stale-row.ts'))
    await settle()

    expect(harness.text()).not.toContain('stale-row.ts')
  })

  it('clears the rows when the load fails', async () => {
    const forA = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()
    forA.resolve(changes('from-thread-a.ts'))
    await settle()
    expect(harness.text()).toContain('from-thread-a.ts')

    const failed = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(failed.promise)
    useCodeSessionStore.setState({ lastSeq: useCodeSessionStore.getState().lastSeq + 1 })
    await settle()
    failed.reject(new Error('changes unavailable'))
    await settle()

    expect(harness.text()).not.toContain('from-thread-a.ts')
  })

  it('clears the rows when the thread selection is dropped', async () => {
    const forA = deferred<WorkspaceChangesResponse>()
    changesMock.mockReturnValueOnce(forA.promise)
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()
    forA.resolve(changes('from-thread-a.ts'))
    await settle()
    expect(harness.text()).toContain('from-thread-a.ts')

    changesMock.mockReturnValueOnce(pending<WorkspaceChangesResponse>())
    useCodeSessionStore.setState({ activeThreadId: null })
    await settle()

    expect(harness.text()).not.toContain('from-thread-a.ts')

    // Selecting another thread must not resurrect the cleared rows.
    useCodeSessionStore.setState({ activeThreadId: THREAD_B })
    await settle()
    expect(harness.text()).not.toContain('from-thread-a.ts')
  })
})

describe('ChangesPanel diff expansion', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    changesMock.mockReset()
    viewMock.mockReset()
    diffMock.mockReset()
    viewMock.mockResolvedValue(workspaceView)
    useCodeSessionStore.getState().resetItems()
    useCodeSessionStore.setState({ activeThreadId: null })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('parses a file patch only while its row is expanded', async () => {
    diffMock.mockResolvedValue({
      ...workspaceDiff,
      unstaged: [{
        path: 'edited.ts',
        status: 'M',
        binary: false,
        truncated: false,
        patch: `--- a/edited.ts\n+++ b/edited.ts\n@@ -1 +1 @@\n-old line\n+new line\n`
      }]
    })
    changesMock.mockResolvedValue(changes('edited.ts'))
    useCodeSessionStore.setState({ activeThreadId: THREAD_A })

    harness = await mountWithProviders(<ChangesPanel />)
    await settle()
    expect(harness.text()).toContain('edited.ts')
    // Collapsed: the patch body must not be parsed into the DOM.
    expect(harness.text()).not.toContain('new line')

    const details = harness.host.querySelector('details.ark-workspace-diff__file')
    expect(details).not.toBeNull()
    await act(async () => {
      ;(details as HTMLDetailsElement).open = true
      // jsdom fires the toggle event asynchronously after the property change.
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    await settle()
    expect(harness.text()).toContain('new line')
  })
})
