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
/**
 * While a turn is running, this connection must keep delivering data frames;
 * only transport keep-alive comments arriving past this bound means the
 * subscription is a zombie (the store dropped it as a slow subscriber) and is
 * torn down deliberately so the resume handshake replays the missed events.
 */
const DATA_WATCHDOG_MS = 45_000
/** A connection that delivered frames and stayed up this long resets the backoff. */
const STABLE_CONNECTION_MS = 5_000

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
  // Only used to label the next attempt as `reconnecting` rather than
  // `connecting`. It must not gate per-connection `open` reporting.
  let hasEverOpened = false
  // Set while the thread has an in-flight turn and cleared on its terminal
  // event. Only a running turn arms the data-frame watchdog below: a quiet
  // thread legitimately goes minutes without any frame, while a running turn
  // must keep producing them.
  let turnActive = false

  while (!options.signal.aborted) {
    options.onStatus(hasEverOpened ? 'reconnecting' : 'connecting')

    let idleTimer: ReturnType<typeof setTimeout> | null = null
    const controller = new AbortController()
    const forwardAbort = (): void => controller.abort()
    options.signal.addEventListener('abort', forwardAbort, { once: true })

    const armWatchdog = (): void => {
      if (idleTimer) clearTimeout(idleTimer)
      idleTimer = setTimeout(() => controller.abort(), IDLE_WATCHDOG_MS)
    }

    // Data-frame watchdog. Byte liveness cannot see a connection whose
    // subscriber entry was dropped server-side: keep-alive comments keep
    // feeding `armWatchdog` while no event ever arrives again, and a running
    // turn would stay invisible until the next page load. While the turn is
    // active this timer only resets on parsed frames, so a comments-only
    // stream aborts and resumes from the last seq (replay fills the gap).
    let dataTimer: ReturnType<typeof setTimeout> | null = null
    const disarmDataFrameWatchdog = (): void => {
      if (dataTimer) {
        clearTimeout(dataTimer)
        dataTimer = null
      }
    }
    const armDataFrameWatchdog = (): void => {
      if (!turnActive || options.signal.aborted) return
      disarmDataFrameWatchdog()
      dataTimer = setTimeout(() => controller.abort(), DATA_WATCHDOG_MS)
    }

    const connectedAt = Date.now()
    let framesSeen = 0
    let openedThisConnection = false

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
        framesSeen += 1
        // Every successful connection reports `open`, including after a
        // reconnect: the composer gates sending on `streamStatus === 'open'`,
        // so a stale `reconnecting` would leave the thread unable to send
        // until the page is reloaded.
        if (!openedThisConnection) {
          openedThisConnection = true
          hasEverOpened = true
          options.onStatus('open')
        }

        const parsed = parseEventFrame(frame)
        if (!parsed) continue
        // A parsed frame is proof of data liveness; comment-only chunks are
        // not and never reset this timer (they keep feeding the byte
        // watchdog above).
        armDataFrameWatchdog()

        // Heartbeats carry a seq and must advance the resume point, but they are
        // not display events; `sse_idle_timeout` asks us to resume deliberately.
        if (parsed.event.kind === 'sse_idle_timeout') {
          lastSeq = Math.max(lastSeq, parsed.seq)
          controller.abort()
          break
        }

        if (parsed.seq > lastSeq) lastSeq = parsed.seq
        if (parsed.event.kind === 'turn_started' || parsed.event.kind === 'turn_steered') {
          turnActive = true
          armDataFrameWatchdog()
        } else if (
          parsed.event.kind === 'turn_completed' ||
          parsed.event.kind === 'turn_failed' ||
          parsed.event.kind === 'turn_aborted'
        ) {
          turnActive = false
          disarmDataFrameWatchdog()
        }
        options.onEvent(parsed.event)
      }
    } catch (error) {
      if (!options.signal.aborted) options.onError?.(error)
    } finally {
      if (idleTimer) clearTimeout(idleTimer)
      disarmDataFrameWatchdog()
      options.signal.removeEventListener('abort', forwardAbort)
    }

    if (options.signal.aborted) break
    // A fast-flapping connection must keep backing off. Only a connection that
    // delivered frames and stayed up for a while resets the schedule: resetting
    // on every frame made the exponential backoff unreachable and produced a
    // reconnect storm while the runtime was down.
    const stable = framesSeen > 0 && Date.now() - connectedAt >= STABLE_CONNECTION_MS
    attempt = stable ? 0 : attempt + 1
    options.onStatus('reconnecting')
    await sleep(backoffMs(attempt), options.signal)
  }

  options.onStatus('closed')
}
