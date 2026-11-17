import type { StreamStatus } from '../api/events-api'

/**
 * A new turn is safe only after the event stream can display its lifecycle.
 * `pendingStart` marks the composer that ships with the new-task page: it has
 * no thread yet, so its send action creates one instead of starting a turn and
 * the SSE gate simply does not apply to it.
 */
export function canSendToThread(
  threadId: string | null,
  sending: boolean,
  streamStatus: StreamStatus,
  pendingStart = false
): boolean {
  if (pendingStart) return !sending
  return threadId !== null && !sending && streamStatus === 'open'
}

/** Drafting is local and remains safe while the event stream reconnects. */
export function canEditComposer(threadId: string | null, sending: boolean): boolean {
  return threadId !== null && !sending
}
