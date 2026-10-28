import type { StreamStatus } from '../api/events-api'

/** A new turn is safe only after the event stream can display its lifecycle. */
export function canSendToThread(threadId: string | null, sending: boolean, streamStatus: StreamStatus): boolean {
  return threadId !== null && !sending && streamStatus === 'open'
}

/** Drafting is local and remains safe while the transport reconnects. */
export function canEditComposer(threadId: string | null, sending: boolean): boolean {
  return threadId !== null && !sending
}
