// @vitest-environment jsdom
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { deleteThreadsByWorkspace } from '../api/threads-api'
import { getWorkspaceView } from '../../settings/api/workspace-api'
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
  getWorkspaceView: vi.fn().mockResolvedValue({ active: { path: '/tmp/ark', generation: 0 }, recent: [], generation: 0 })
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
    // restoreAllMocks in afterEach wipes the module factory's own mockResolvedValue;
    // re-seed it so the panel's workspace probe never rejects.
    vi.mocked(getWorkspaceView).mockResolvedValue({ active: { path: '/tmp/ark', generation: 0 }, recent: [], generation: 0 })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  async function openConfirmGate(): Promise<void> {
    harness = await mountWithProviders(<ThreadListPanel />)
    await settle()
    const remove = harness.host.querySelector<HTMLButtonElement>('.ark-projectgroup__action.is-danger')!
    expect(remove).toBeTruthy()
    await act(async () => { remove.click() })
    await settle()
  }

  it('opens an in-app confirm dialog before deleting and does nothing on cancel', async () => {
    // The gate is our own dialog, not window.confirm: the native confirm is
    // swallowed by the desktop WebView and deletes ran through unchecked.
    const nativeConfirm = vi.spyOn(window, 'confirm')
    await openConfirmGate()

    const dialog = harness!.host.querySelector('.ark-confirmdialog [role="dialog"]')
    expect(dialog).toBeTruthy()
    expect(dialog!.getAttribute('aria-label')).toContain('ark')
    expect(nativeConfirm).not.toHaveBeenCalled()
    expect(deleteThreadsByWorkspace).not.toHaveBeenCalled()

    const cancel = harness!.host.querySelector<HTMLButtonElement>('.ark-confirmdialog__actions button')!
    await act(async () => { cancel.click() })
    await settle()
    expect(harness!.host.querySelector('.ark-confirmdialog')).toBeNull()
    expect(deleteThreadsByWorkspace).not.toHaveBeenCalled()
  })

  it('deletes the project only after the dialog is confirmed', async () => {
    vi.mocked(deleteThreadsByWorkspace).mockResolvedValue({ deleted: [thread().thread_id], skipped: [] })
    await openConfirmGate()

    const buttons = harness!.host.querySelectorAll<HTMLButtonElement>('.ark-confirmdialog__actions button')
    const confirm = buttons[buttons.length - 1]!
    await act(async () => { confirm.click() })
    await settle()

    expect(deleteThreadsByWorkspace).toHaveBeenCalledTimes(1)
    expect(deleteThreadsByWorkspace).toHaveBeenCalledWith('/tmp/ark')
    expect(harness!.host.querySelector('.ark-confirmdialog')).toBeNull()
  })
})
