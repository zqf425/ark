/**
 * Transport-level SSE subscription for one thread.
 *
 * Responsibilities kept here (and only here): framing, `seq` bookkeeping, the
 * replay/resume handshake and backoff. Interpreting events is the reducer's job,
 * so this module never inspects `kind` beyond `heartbeat`/`sse_idle_timeout`.
 */
import type { RuntimeEvent } from '../../../shared/contracts'
import { stream } from '../../../shared/api/client'

export type StreamStatus = 'connecting' | 'open' | 'reconnecting' | 'closed'

export type ThreadStreamOptions = {
  threadId: string
  /** Resume point: the server sends events with `seq > sinceSeq`. */
  sinceSeq: number
  signal: AbortSignal
  onEvent: (event: RuntimeEvent) => void
  onStatus: (status: StreamStatus) => void
  onError?: (error: unknown) => void
}

const MAX_BACKOFF_MS = 10_000
const IDLE_WATCHDOG_MS = 75_000

function backoffMs(attempt: number): number {
  const base = Math.min(MAX_BACKOFF_MS, 500 * 2 ** Math.min(attempt, 5))
  // Jitter keeps several reconnects from synchronising after a runtime restart.
  return Math.round(base * (0.75 + Math.random() * 0.5))
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(finish, ms)
    function finish(): void {
      clearTimeout(timer)
      signal.removeEventListener('abort', finish)
      resolve()
    }
    signal.addEventListener('abort', finish, { once: true })
  })
}

type ParsedFrame = { event: RuntimeEvent; seq: number } | null

function parseEventFrame(frame: { id: string | null; event: string | null; data: string }): ParsedFrame {
  if (!frame.data) return null
  let payload: unknown
  try {
    payload = JSON.parse(frame.data)
  } catch {
    return null
  }
  if (!payload || typeof payload !== 'object') return null

  const event = payload as RuntimeEvent
  const headerSeq = frame.id === null ? Number.NaN : Number(frame.id)
  const seq = Number.isFinite(headerSeq) ? headerSeq : Number(event.seq)
  if (!Number.isFinite(seq)) return null
  return { event, seq }
}

/**
 * Keeps one logical subscription alive across transport failures.
 *
 * Resolves only when `signal` aborts: reconnection is the default behaviour, so
 * the caller never needs its own retry loop.
 */
export async function subscribeThreadEvents(options: ThreadStreamOptions): Promise<void> {
  let lastSeq = options.sinceSeq
  let attempt = 0
  let opened = false

  while (!options.signal.aborted) {
    options.onStatus(opened ? 'reconnecting' : 'connecting')

    let idleTimer: ReturnType<typeof setTimeout> | null = null
    const controller = new AbortController()
    const forwardAbort = (): void => controller.abort()
    options.signal.addEventListener('abort', forwardAbort, { once: true })

    const armWatchdog = (): void => {
      if (idleTimer) clearTimeout(idleTimer)
      idleTimer = setTimeout(() => controller.abort(), IDLE_WATCHDOG_MS)
    }

    try {
      armWatchdog()
      const frames = stream(`/v1/threads/${options.threadId}/events`, {
        query: { since_seq: lastSeq },
        signal: controller.signal,
        // Liveness is measured in bytes, not frames: a quiet thread only sees
        // `: keep-alive` comments, which parse into no frame and would
        // otherwise let the watchdog abort a perfectly healthy connection.
        onActivity: armWatchdog
      })

      for await (const frame of frames) {
        armWatchdog()
        if (!opened) {
          opened = true
          options.onStatus('open')
        }
        attempt = 0

        const parsed = parseEventFrame(frame)
        if (!parsed) continue

        // Heartbeats carry a seq and must advance the resume point, but they are
        // not display events; `sse_idle_timeout` asks us to resume deliberately.
        if (parsed.event.kind === 'sse_idle_timeout') {
          lastSeq = Math.max(lastSeq, parsed.seq)
          controller.abort()
          break
        }

        if (parsed.seq > lastSeq) lastSeq = parsed.seq
        options.onEvent(parsed.event)
      }
    } catch (error) {
      if (!options.signal.aborted) options.onError?.(error)
    } finally {
      if (idleTimer) clearTimeout(idleTimer)
      options.signal.removeEventListener('abort', forwardAbort)
    }

    if (options.signal.aborted) break
    options.onStatus('reconnecting')
    await sleep(backoffMs(attempt), options.signal)
    attempt += 1
  }

  options.onStatus('closed')
}
