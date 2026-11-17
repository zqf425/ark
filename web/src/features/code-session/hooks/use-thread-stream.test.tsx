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
import type { RuntimeEvent, ThreadDetail, ThreadItem } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { getThread, getThreadUsage } from '../api/threads-api'
import { subscribeThreadEvents } from '../api/events-api'
import { useCodeSessionStore } from '../state/store'
import { useThreadStream } from './use-thread-stream'
import { resetTurnEndNotifications } from './turn-end-toast'

vi.mock('../api/threads-api', () => ({ getThread: vi.fn(), getThreadUsage: vi.fn() }))
vi.mock('../api/events-api', () => ({ subscribeThreadEvents: vi.fn(() => new Promise<void>(() => undefined)) }))

const getThreadMock = vi.mocked(getThread)
const getThreadUsageMock = vi.mocked(getThreadUsage)
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
      running: false,
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

/** Seed one settled rail row so terminal toasts have a title to name. */
function seedRail(threadId: string): void {
  useCodeSessionStore.setState({
    threads: [{
      thread_id: threadId,
      title: 'rail',
      created_at: '2026-10-28T00:00:00Z',
      updated_at: '2026-10-28T00:00:00Z',
      archived: false,
      running: false,
      last_event_at: '2026-10-29T00:00:00Z'
    }],
    activeThreadId: threadId
  })
}

describe('useThreadStream thread ownership', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    getThreadMock.mockReset()
    getThreadUsageMock.mockReset()
    // The usage seed is best effort; tests only assert the stream projection.
    getThreadUsageMock.mockReturnValue(new Promise<never>(() => undefined))
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
    useCodeSessionStore.getState().seedItems([item({})], 7, { pending_approvals: [], pending_user_inputs: [] })

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

  it('seeds the usage aggregate after the detail load', async () => {
    getThreadMock.mockImplementation(() => Promise.resolve(detail(THREAD_A, 41, [item({})])))
    getThreadUsageMock.mockResolvedValue({
      thread_id: THREAD_A,
      turns: 1,
      samples: 2,
      prompt_tokens: 100,
      completion_tokens: 20,
      total_tokens: 120
    })
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    const usage = useCodeSessionStore.getState().usage
    expect(usage?.totalTokens).toBe(120)
    expect(usage?.samples).toBe(2)
  })

  it('leaves the usage channel empty when the usage seed fails', async () => {
    getThreadMock.mockImplementation(() => Promise.resolve(detail(THREAD_A, 41, [item({})])))
    getThreadUsageMock.mockRejectedValue(new Error('usage unavailable'))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    expect(useCodeSessionStore.getState().usage).toBeNull()
    // The stream itself must be unaffected by the failed seed.
    expect(useCodeSessionStore.getState().itemOrder).toHaveLength(1)
  })

  it('re-seeds usage per thread without leaking values across switches', async () => {
    getThreadMock.mockImplementation((threadId) => Promise.resolve(
      threadId === THREAD_A
        ? detail(THREAD_A, 41, [item({})])
        : detail(THREAD_B, 9, [])
    ))
    getThreadUsageMock.mockImplementation((threadId) => Promise.resolve(
      threadId === THREAD_A
        ? { thread_id: THREAD_A, turns: 1, samples: 2, prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 }
        : { thread_id: THREAD_B, turns: 1, samples: 1, prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 }
    ))

    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()
    expect(useCodeSessionStore.getState().usage?.totalTokens).toBe(120)

    await harness.render(<Probe threadId={THREAD_B} />)
    await settle()
    // B's own aggregate, never A's leftover.
    expect(useCodeSessionStore.getState().usage?.totalTokens).toBe(7)
  })
})

/** Wire the mocked subscription's onEvent sink into a module-level variable. */
let onEvent: ((event: RuntimeEvent) => void) | null = null

function armEventSink(): void {
  onEvent = null
  subscribeMock.mockImplementation((_options) => {
    onEvent = _options.onEvent
    return new Promise<void>(() => undefined)
  })
}

describe('useThreadStream rail activity', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    getThreadMock.mockReset()
    getThreadUsageMock.mockReset()
    getThreadUsageMock.mockReturnValue(new Promise<never>(() => undefined))
    armEventSink()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('a synthetic heartbeat frame does not reorder the rail', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    // The server's opening frame for an empty backlog is stamped `now`.
    onEvent?.({ kind: 'heartbeat', seq: 7, thread_id: THREAD_A, timestamp: '2026-11-20T00:00:00Z' } as never)

    const row = useCodeSessionStore.getState().threads.find((t) => t.thread_id === THREAD_A)
    expect(row?.last_event_at).toBe('2026-10-29T00:00:00Z')
  })

  it('a real milestone event still advances the rail activity key', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    onEvent?.({
      kind: 'turn_completed',
      seq: 8,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z'
    } as never)

    const row = useCodeSessionStore.getState().threads.find((t) => t.thread_id === THREAD_A)
    expect(row?.last_event_at).toBe('2026-11-20T00:00:00Z')
    expect(row?.last_seq).toBe(8)
  })

  it('a user-posted turn boundary (turn_started) advances the rail activity key', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    onEvent?.({
      kind: 'turn_started',
      seq: 8,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z',
      auto_continuation: false,
      attachment_ids: [],
      attachment_outcomes: []
    } as never)

    const row = useCodeSessionStore.getState().threads.find((t) => t.thread_id === THREAD_A)
    expect(row?.last_event_at).toBe('2026-11-20T00:00:00Z')
    expect(row?.last_seq).toBe(8)
  })

  it('in-flight events (delta, tool call) advance the seq but never the activity key or order', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()
    const before = useCodeSessionStore.getState().threads

    onEvent?.({
      kind: 'assistant_text_delta',
      seq: 8,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      item_id: '00000000-0000-0000-0000-0000000000i1',
      timestamp: '2026-11-20T00:00:00Z',
      delta: 'hello'
    } as never)
    onEvent?.({
      kind: 'tool_call_started',
      seq: 9,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-21T00:00:00Z',
      call_id: 'call-1',
      tool: 'bash',
      arguments: {}
    } as never)

    const row = useCodeSessionStore.getState().threads.find((t) => t.thread_id === THREAD_A)
    // The watermark advanced; the activity key did not, so the row keeps its
    // position (its key is older than B's in a real multi-thread rail).
    expect(row?.last_seq).toBe(9)
    expect(row?.last_event_at).toBe('2026-10-29T00:00:00Z')
    // Same order, same key: only the watermark moved.
    expect(useCodeSessionStore.getState().threads.map((t) => t.thread_id)).toEqual(before.map((t) => t.thread_id))
  })

  it('selecting a thread (detail seed with a fresher last_event_at) does not lift its rail row', async () => {
    // Rail row seeded from a list snapshot taken before the thread's latest
    // in-flight events; the detail is therefore "fresher" than the row.
    seedRail(THREAD_A)
    const detailRow = {
      ...detail(THREAD_A, 41, []).summary,
      last_event_at: '2026-11-25T00:00:00Z'
    }
    getThreadMock.mockResolvedValue({ ...detail(THREAD_A, 41, []), summary: detailRow })
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    // The row keeps the snapshot's key: clicking a running thread must not
    // reorder the rail. Only turn-boundary events or a list refresh may.
    const row = useCodeSessionStore.getState().threads.find((t) => t.thread_id === THREAD_A)
    expect(row?.last_event_at).toBe('2026-10-29T00:00:00Z')
    expect(row?.last_seq).toBe(41)
  })
})

describe('useThreadStream turn-end notifications', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    getThreadMock.mockReset()
    getThreadUsageMock.mockReset()
    getThreadUsageMock.mockReturnValue(new Promise<never>(() => undefined))
    armEventSink()
    useCodeSessionStore.getState().resetItems()
    // The toast dedupe is a module-level map that outlives one test; the
    // rail-activity describe above fires the same terminal frame.
    resetTurnEndNotifications()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('a turn_completed frame toasts once with the thread title', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    onEvent?.({
      kind: 'turn_completed',
      seq: 8,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z'
    } as never)
    await settle()

    expect(document.body.textContent).toContain('会话「rail」已完成回复')

    // A reconnect replays the same terminal frame: still one toast.
    onEvent?.({
      kind: 'turn_completed',
      seq: 8,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z'
    } as never)
    await settle()

    expect(document.body.querySelectorAll('.ark-toast').length).toBe(1)
  })

  it('turn_failed toasts with the failure wording; other threads stay silent', async () => {
    seedRail(THREAD_A)
    getThreadMock.mockResolvedValue(detail(THREAD_A, 7, []))
    harness = await mountWithProviders(<Probe threadId={THREAD_A} />)
    await settle()

    onEvent?.({
      kind: 'turn_failed',
      seq: 8,
      thread_id: THREAD_B,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z',
      message: 'boom',
      model_request_failed: false
    } as never)
    await settle()
    expect(document.body.querySelector('.ark-toast')).toBeNull()

    onEvent?.({
      kind: 'turn_failed',
      seq: 9,
      thread_id: THREAD_A,
      turn_id: TURN_A,
      timestamp: '2026-11-20T00:00:00Z',
      message: 'boom',
      model_request_failed: false
    } as never)
    await settle()
    expect(document.body.textContent).toContain('会话「rail」回合失败')
  })
})
