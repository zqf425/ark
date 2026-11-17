// @vitest-environment jsdom
/**
 * Turn-end notifications: the in-page toast and, only while the window lacks
 * focus, the desktop shell's system notification.
 *
 * The system path is the reason this module exists beyond the toast: a turn
 * finishing while the user reads another app must still surface (Windows
 * toast / macOS banner). Focus is sampled at delivery time, so the tests stub
 * `document.hasFocus` per case instead of simulating focus events.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const desktopMocks = vi.hoisted(() => ({
  isDesktopShell: vi.fn(() => false),
  pickFolder: vi.fn(async () => null as string | null),
  saveFile: vi.fn(async (_path: string, _bytes: Uint8Array) => ''),
  postSystemNotification: vi.fn(async (_title: string, _body: string) => true)
}))

vi.mock('../../../shared/desktop', () => desktopMocks)
vi.mock('../../../shared/ui', () => ({ pushToastProgrammatic: vi.fn() }))

import { pushToastProgrammatic } from '../../../shared/ui'
import { useCodeSessionStore } from '../state/store'
import {
  notifyTurnEnded,
  resetTurnEndNotifications,
  type TurnEndOutcome
} from './turn-end-toast'

const toastMock = vi.mocked(pushToastProgrammatic)
const systemMock = desktopMocks.postSystemNotification

const THREAD_ID = '00000000-0000-0000-0000-00000000000a'

function seedThread(title = 'rail'): void {
  useCodeSessionStore.setState({
    threads: [
      {
        thread_id: THREAD_ID,
        title,
        created_at: '2026-10-28T00:00:00Z',
        updated_at: '2026-10-28T00:00:00Z',
        archived: false,
        running: false,
        last_seq: 7
      }
    ]
  })
}

describe('turn-end notifications', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    resetTurnEndNotifications()
    useCodeSessionStore.getState().resetItems()
    seedThread()
  })

  it('toasts and skips the system notification while the window is focused', () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(true)

    expect(notifyTurnEnded({ threadId: THREAD_ID, outcome: 'completed', endTime: 't1' })).toBe(true)

    expect(toastMock).toHaveBeenCalledTimes(1)
    expect(toastMock.mock.calls[0]?.[1]).toContain('会话「rail」已完成回复')
    expect(systemMock).not.toHaveBeenCalled()
  })

  it('adds a system notification when the window is not focused', async () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(false)

    expect(notifyTurnEnded({ threadId: THREAD_ID, outcome: 'completed', endTime: 't1' })).toBe(true)

    expect(toastMock).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(systemMock).toHaveBeenCalledTimes(1))
    const [title, body] = systemMock.mock.calls[0] ?? ['', '']
    expect(title).toBe('会话完成')
    expect(body).toBe('会话「rail」已完成回复')
  })

  it('dedupes the system notification with the same durable key as the toast', async () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(false)

    notifyTurnEnded({ threadId: THREAD_ID, outcome: 'completed', endTime: 't1' })
    await vi.waitFor(() => expect(systemMock).toHaveBeenCalledTimes(1))

    // Replay from a reconnect or the list poll: no second surface.
    expect(notifyTurnEnded({ threadId: THREAD_ID, outcome: 'completed', endTime: 't1' })).toBe(false)
    await Promise.resolve()
    expect(systemMock).toHaveBeenCalledTimes(1)
    expect(toastMock).toHaveBeenCalledTimes(1)
  })

  it('maps every outcome to its own system title', async () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(false)
    const cases: Array<[TurnEndOutcome, string]> = [
      ['completed', '会话完成'],
      ['failed', '会话失败'],
      ['aborted', '会话已中止'],
      ['ended', '会话已结束']
    ]
    let index = 0
    for (const [outcome, title] of cases) {
      systemMock.mockClear()
      notifyTurnEnded({ threadId: THREAD_ID, outcome, endTime: `t${index}` })
      await vi.waitFor(() => expect(systemMock).toHaveBeenCalledTimes(1))
      expect(systemMock.mock.calls[0]?.[0]).toBe(title)
      index += 1
    }
  })

  it('keeps the toast when the shell refuses the notification', async () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(false)
    systemMock.mockResolvedValue(false)

    expect(notifyTurnEnded({ threadId: THREAD_ID, outcome: 'ended', endTime: 't9' })).toBe(true)

    expect(toastMock).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(systemMock).toHaveBeenCalledTimes(1))
    await expect(systemMock.mock.results[0]?.value).resolves.toBe(false)
  })

  it('does not surface anything for an unknown thread', () => {
    vi.spyOn(document, 'hasFocus').mockReturnValue(false)

    expect(notifyTurnEnded({ threadId: 'missing', outcome: 'ended' })).toBe(false)
    expect(toastMock).not.toHaveBeenCalled()
    expect(systemMock).not.toHaveBeenCalled()
  })
})
