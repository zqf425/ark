// @vitest-environment jsdom
/**
 * A thread switch owns the whole canvas projection.
 *
 * The hook stays mounted while the active thread changes, so anything it fails
 * to drop is shown as if it belonged to the new thread: A's messages, turn
 * states and approval/user-input gates would stay on screen (and stay
 * clickable) during B's load, and a failed B load would leave them there for
 * good. Both branches must end with an empty projection.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadDetail, ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { getThread } from '../api/threads-api'
import { subscribeThreadEvents } from '../api/events-api'
import { useCodeSessionStore } from '../state/store'
import { useThreadStream } from './use-thread-stream'

vi.mock('../api/threads-api', () => ({ getThread: vi.fn() }))
vi.mock('../api/events-api', () => ({ subscribeThreadEvents: vi.fn(() => new Promise<void>(() => undefined)) }))

const getThreadMock = vi.mocked(getThread)
const subscribeMock = vi.mocked(subscribeThreadEvents)

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'
const TURN_A = '00000000-0000-0000-0000-0000000000ta'

function item(overrides: Partial<ThreadItem>): ThreadItem {
  return {
    item_id: '00000000-0000-0000-0000-0000000000i1',
    turn_id: TURN_A,
    kind: 'assistant_message',
    status: 'completed',
    created_at: '2026-10-28T00:00:00Z',
    text: 'thread A message',
    ...overrides
  }
}

function detail(threadId: string, lastSeq: number, items: ThreadItem[], approvals: ThreadDetail['runtime_snapshot']['pending_approvals'] = []): ThreadDetail {
  return {
    summary: {
      thread_id: threadId,
      title: threadId,
      created_at: '2026-10-28T00:00:00Z',
      updated_at: '2026-10-28T00:00:00Z',
      last_seq: lastSeq,
      archived: false
    },
    items,
    child_ids: [],
    child_completions: [],
    attachment_ids: [],
    runtime: {},
    runtime_snapshot: { pending_approvals: approvals, pending_user_inputs: [] }
  }
}

function Probe({ threadId }: { threadId: string | null }): null {
  useThreadStream(threadId)
  return null
}

describe('useThreadStream thread ownership', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    getThreadMock.mockReset()
    subscribeMock.mockClear()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('drops the previous thread projection while the next thread loads', async () => {
    getThreadMock.mockImplementation(() => new Promise<ThreadDetail>(() => undefined))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    // A is on screen: rows plus a gate the user could still answer.
    useCodeSessionStore.getState().seedItems(
      [item({})],
      7,
      [],
      {
        pending_approvals: [{
          approval_id: 'approval-a',
          turn_id: TURN_A,
          tool: 'bash',
          summary: 'run command',
          requested_at: '2026-10-28T00:00:00Z'
        }],
        pending_user_inputs: []
      }
    )
    expect(useCodeSessionStore.getState().itemOrder).toHaveLength(1)

    // Switching to B must clear on the commit that starts B's load, not when B
    // finally answers.
    await harness.render(<Probe threadId={THREAD_B} />)
    await settle()

    const state = useCodeSessionStore.getState()
    expect(state.detailStatus).toBe('loading')
    expect(state.itemOrder).toEqual([])
    expect(state.itemsById).toEqual({})
    expect(state.approvals).toEqual({})
    expect(state.userInputs).toEqual({})
    expect(state.activeTurnId).toBeNull()
  })

  it('clears the projection when the new thread detail fails', async () => {
    getThreadMock.mockImplementation(() => new Promise<ThreadDetail>(() => undefined))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()
    useCodeSessionStore.getState().seedItems([item({})], 7, [], { pending_approvals: [], pending_user_inputs: [] })

    getThreadMock.mockImplementation(() => Promise.reject(new Error('thread B unavailable')))
    await harness.render(<Probe threadId={THREAD_B} />)
    await settle()

    const state = useCodeSessionStore.getState()
    expect(state.detailStatus).toBe('error')
    expect(state.detailError).toContain('thread B unavailable')
    // A's history must not survive under B's error.
    expect(state.itemOrder).toEqual([])
    expect(state.turnState).toEqual({})
    expect(state.lastSeq).toBe(0)
  })

  it('subscribes from the new thread watermark, never the previous one', async () => {
    getThreadMock.mockImplementation((threadId) => Promise.resolve(
      threadId === THREAD_A
        ? detail(THREAD_A, 41, [item({})])
        : detail(THREAD_B, 9, [item({ item_id: '00000000-0000-0000-0000-0000000000i2', text: 'thread B message' })])
    ))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()
    expect(subscribeMock).toHaveBeenLastCalledWith(expect.objectContaining({ threadId: THREAD_A, sinceSeq: 41 }))

    await harness.render(<Probe threadId={THREAD_B} />)
    await settle()

    expect(subscribeMock).toHaveBeenLastCalledWith(expect.objectContaining({ threadId: THREAD_B, sinceSeq: 9 }))
    const state = useCodeSessionStore.getState()
    expect(state.lastSeq).toBe(9)
    expect(state.itemOrder).toHaveLength(1)
    expect(state.itemsById['00000000-0000-0000-0000-0000000000i2']?.text).toBe('thread B message')
  })
})
