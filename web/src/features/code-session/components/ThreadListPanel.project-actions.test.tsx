// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { deleteThreadsByWorkspace } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import { ThreadListPanel } from './ThreadListPanel'

vi.mock('../api/threads-api', () => ({
  listThreads: vi.fn(),
  createThread: vi.fn(),
  deleteThread: vi.fn(),
  deleteThreadsByWorkspace: vi.fn(),
  forkThread: vi.fn(),
  updateThread: vi.fn(),
  exportThread: vi.fn()
}))

vi.mock('../hooks/use-thread-running', () => ({
  useThreadRunning: () => undefined
}))

vi.mock('../../settings/api/workspace-api', () => ({
  getWorkspaceView: vi.fn().mockResolvedValue({ active: '/tmp/ark', roots: [] })
}))

vi.mock('./ScheduleCreateDialog', () => ({
  ScheduleCreateDialog: () => null
}))

function thread(): ThreadSummary {
  return {
    thread_id: '00000000-0000-0000-0000-00000000000a',
    title: '会话',
    workspace: '/tmp/ark',
    created_at: '2026-10-28T00:00:00Z',
    updated_at: '2026-10-28T00:00:00Z',
    archived: false,
    running: false
  }
}

describe('project group delete confirmation', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    useCodeSessionStore.getState().setThreads([thread()])
    vi.mocked(deleteThreadsByWorkspace).mockReset()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('asks for confirmation before deleting a project and does nothing on cancel', async () => {
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false)
    harness = await mountWithProviders(<ThreadListPanel />)
    await settle()

    const remove = harness.host.querySelector<HTMLButtonElement>('.ark-projectgroup__action.is-danger')!
    expect(remove).toBeTruthy()
    await act(async () => { remove.click() })
    await settle()

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(confirm.mock.calls[0]![0]).toContain('ark')
    expect(deleteThreadsByWorkspace).not.toHaveBeenCalled()
  })
})
