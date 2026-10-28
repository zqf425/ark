import type {
  ArkErrorCode,
  ChildCompletionNotice,
  ItemId,
  ThreadId,
  ThreadItem,
  ThreadSummary,
  TurnId,
  TurnState
} from '../../../shared/contracts'
import type { StreamStatus } from '../api/events-api'

export type { ChildCompletionNotice } from '../../../shared/contracts'

/** One runtime `error` event surfaced inline in the stream. */
export type ErrorLine = {
  seq: number
  timestamp: string
  code: ArkErrorCode | 'unknown'
  message: string
}

/**
 * Gates are the "few control events" the renderer is still allowed to read
 * (design §7.10.3): they carry the id the user must answer with.
 */
export type PendingApproval = {
  approval_id: string
  turn_id: TurnId
  tool: string
  summary: string
  status: 'pending' | 'approved' | 'rejected'
  requested_at: string
}

export type PendingUserInput = {
  request_id: string
  turn_id: TurnId
  prompt: string
  status: 'pending' | 'answered'
  answer?: string
  requested_at: string
}

export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error'

/** Everything the reducer owns, addressable as a plain value. */
export type ItemsSnapshot = {
  itemsById: Record<ItemId, ThreadItem>
  itemOrder: ItemId[]
  turnState: Record<TurnId, TurnState>
  activeTurnId: TurnId | null
  lastSeq: number
  errors: ErrorLine[]
  approvals: Record<string, PendingApproval>
  userInputs: Record<string, PendingUserInput>
  childCompletions: Record<string, ChildCompletionNotice>
  /** Advances only for child lifecycle events, not every streamed token. */
  childRunRevision: number
}

export type ThreadListSnapshot = {
  threads: ThreadSummary[]
  activeThreadId: ThreadId | null
  listStatus: LoadStatus
  listError: string | null
}

export type StreamSnapshot = {
  detailStatus: LoadStatus
  detailError: string | null
  streamStatus: StreamStatus
  streamError: string | null
}

export type RuntimeSnapshot = {
  online: 'unknown' | 'online' | 'offline'
  healthVersion: string | null
  runtimeVersion: string | null
  dataDir: string | null
  threadCount: number | null
  checkedAt: number | null
  healthError: string | null
  sending: boolean
  interrupting: boolean
  commandError: string | null
}
