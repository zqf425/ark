import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { subscribeThreadEvents, type StreamStatus } from './events-api'

const THREAD_ID = '00000000-0000-0000-0000-000000000001'
const IDLE_WATCHDOG_MS = 75_000
const KEEP_ALIVE_INTERVAL_MS = 15_000
const SINCE_SEQ = 13

type FakeStream = {
  push: (chunk: string) => void
}

/**
 * Replaces `fetch` with hand-fed bodies so the subscription can be driven one
 * transport chunk at a time. Every request is recorded and its body errors out
 * when the request signal aborts, which is how a real aborted SSE behaves.
 */
function stubSseFetch(): { calls: string[]; latest: () => FakeStream | undefined } {
  const calls: string[] = []
  const streams: FakeStream[] = []
  const encoder = new TextEncoder()

  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      calls.push(url)
      const box: { controller?: ReadableStreamDefaultController<Uint8Array> } = {}
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          box.controller = controller
        }
      })
      init?.signal?.addEventListener('abort', () => {
        try {
          box.controller?.error(new DOMException('aborted', 'AbortError'))
        } catch {
          // The test already ended this body.
        }
      })
      streams.push({
        push: (chunk) => box.controller?.enqueue(encoder.encode(chunk))
      })
      return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } })
    })
  )

  return { calls, latest: () => streams[streams.length - 1] }
}

/**
 * Drain pending promise work between chunks. Microtasks are never faked, and
 * advancing by 0ms flushes promise chains without moving the faked watchdog
 * clock, so `fetch -> reader.read -> for await` all settle here.
 */
async function flush(): Promise<void> {
  for (let i = 0; i < 50; i += 1) {
    await Promise.resolve()
    await vi.advanceTimersByTimeAsync(0)
  }
}

function heartbeatFrame(seq: number): string {
  return `id: ${seq}\nevent: heartbeat\ndata: ${JSON.stringify({
    kind: 'heartbeat',
    seq,
    thread_id: THREAD_ID,
    timestamp: '2026-10-28T00:00:00Z'
  })}\n\n`
}

type Harness = {
  controller: AbortController
  statuses: StreamStatus[]
  events: unknown[]
  errors: unknown[]
  done: Promise<void>
}

function start(): Harness {
  const controller = new AbortController()
  const statuses: StreamStatus[] = []
  const events: unknown[] = []
  const errors: unknown[] = []
  const done = subscribeThreadEvents({
    threadId: THREAD_ID,
    sinceSeq: SINCE_SEQ,
    signal: controller.signal,
    onEvent: (event) => events.push(event),
    onStatus: (status) => statuses.push(status),
    onError: (error) => errors.push(error)
  })
  return { controller, statuses, events, errors, done }
}

/** Abort at the end of a test so the subscription loop can settle. */
async function stop(harness: Harness): Promise<void> {
  harness.controller.abort()
  await vi.advanceTimersByTimeAsync(2_000)
  await harness.done
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('subscribeThreadEvents idle watchdog', () => {
  it('keeps the single connection while only keep-alive comments arrive', async () => {
    const fetchStub = stubSseFetch()
    const harness = start()
    await flush()

    fetchStub.latest()?.push(heartbeatFrame(SINCE_SEQ))
    await flush()
    expect(harness.statuses).toEqual(['connecting', 'open'])
    const eventsAfterOpen = harness.events.length

    // Eight keep-alive rounds span 120s, past the 75s window: a watchdog reset
    // only by event frames recycles the connection here.
    for (let round = 0; round < 8; round += 1) {
      fetchStub.latest()?.push(': keep-alive\n\n')
      await flush()
      await vi.advanceTimersByTimeAsync(KEEP_ALIVE_INTERVAL_MS)
      await flush()
      expect(fetchStub.calls).toHaveLength(1)
      expect(harness.statuses).not.toContain('reconnecting')
    }

    // Comments neither surface as events nor advance the resume watermark.
    expect(harness.events).toHaveLength(eventsAfterOpen)
    expect(harness.errors).toEqual([])

    await stop(harness)
  })

  it('still reconnects from the last seq after true silence past the threshold', async () => {
    const fetchStub = stubSseFetch()
    const harness = start()
    await flush()

    fetchStub.latest()?.push(heartbeatFrame(SINCE_SEQ))
    await flush()
    fetchStub.latest()?.push(': keep-alive\n\n')
    await flush()

    await vi.advanceTimersByTimeAsync(IDLE_WATCHDOG_MS)
    await flush()
    await vi.advanceTimersByTimeAsync(2_000)
    await flush()

    expect(fetchStub.calls).toHaveLength(2)
    expect(fetchStub.calls[1]).toContain(`since_seq=${SINCE_SEQ}`)
    expect(harness.statuses).toContain('reconnecting')

    await stop(harness)
  })

  it('still honours an explicit sse_idle_timeout control frame', async () => {
    const fetchStub = stubSseFetch()
    const harness = start()
    await flush()

    fetchStub.latest()?.push(heartbeatFrame(SINCE_SEQ))
    await flush()
    fetchStub.latest()?.push(
      `id: 21\nevent: sse_idle_timeout\ndata: ${JSON.stringify({
        kind: 'sse_idle_timeout',
        seq: 21,
        thread_id: THREAD_ID,
        timestamp: '2026-10-28T00:00:00Z'
      })}\n\n`
    )
    await flush()
    await vi.advanceTimersByTimeAsync(2_000)
    await flush()

    expect(fetchStub.calls).toHaveLength(2)
    expect(fetchStub.calls[1]).toContain('since_seq=21')
    // The control frame is transport-level: it never reaches the reducer.
    expect(harness.events.map((event) => (event as { kind: string }).kind)).toEqual(['heartbeat'])

    await stop(harness)
  })
})
