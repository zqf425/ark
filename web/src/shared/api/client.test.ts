import { afterEach, describe, expect, it, vi } from 'vitest'
import { parseSseFrame, stream, type SseFrame } from './client'

type FakeBody = {
  push: (chunk: string) => void
  close: () => void
}

/**
 * Replaces `fetch` with a body the test feeds by hand, so the stream reader can
 * be driven one chunk at a time without a real runtime.
 */
function stubSseFetch(): FakeBody {
  const box: { controller?: ReadableStreamDefaultController<Uint8Array> } = {}
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      box.controller = controller
    }
  })
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }))
  )
  const encoder = new TextEncoder()
  return {
    push: (chunk) => {
      try {
        box.controller?.enqueue(encoder.encode(chunk))
      } catch {
        // The reader already cancelled or errored this body.
      }
    },
    close: () => {
      try {
        box.controller?.close()
      } catch {
        // Already closed.
      }
    }
  }
}

/** Let the generator advance past its `await` points (real timers here). */
async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await new Promise((resolve) => setTimeout(resolve, 0))
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('parseSseFrame', () => {
  it('parses a comment-only block into no frame', () => {
    expect(parseSseFrame(': keep-alive')).toBeNull()
  })
})

describe('stream activity reporting', () => {
  it('reports activity for a comment-only keep-alive block without emitting a frame', async () => {
    const body = stubSseFetch()
    let activity = 0
    const frames: SseFrame[] = []
    const consume = (async () => {
      for await (const frame of stream('/v1/threads/thread-1/events', {
        onActivity: () => {
          activity += 1
        }
      })) {
        frames.push(frame)
      }
    })()

    await flush()
    body.push(': keep-alive\n\n')
    await flush()

    expect(activity).toBe(1)
    expect(frames).toEqual([])

    body.close()
    await consume
    expect(frames).toEqual([])
  })

  it('reports activity for a real event frame as well', async () => {
    const body = stubSseFetch()
    let activity = 0
    const frames: SseFrame[] = []
    const consume = (async () => {
      for await (const frame of stream('/v1/threads/thread-1/events', {
        onActivity: () => {
          activity += 1
        }
      })) {
        frames.push(frame)
        return
      }
    })()

    await flush()
    body.push('id: 7\nevent: heartbeat\ndata: {"kind":"heartbeat"}\n\n')

    await consume
    expect(frames).toEqual([{ id: '7', event: 'heartbeat', data: '{"kind":"heartbeat"}' }])
    expect(activity).toBe(1)
  })

  it('reports no activity while the transport delivers no bytes at all', async () => {
    const body = stubSseFetch()
    let activity = 0
    const frames: SseFrame[] = []
    const consume = (async () => {
      for await (const frame of stream('/v1/threads/thread-1/events', {
        onActivity: () => {
          activity += 1
        }
      })) {
        frames.push(frame)
      }
    })()

    await flush()
    expect(activity).toBe(0)

    body.close()
    await consume
    expect(activity).toBe(0)
    expect(frames).toEqual([])
  })
})
