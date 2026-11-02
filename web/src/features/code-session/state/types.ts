import type {
  ArkErrorCode,
  LoadStatus,
  ChildCompletionNotice,
  ItemId,
  PendingApprovalView,
  PendingUserInputView,
  ThreadId,
  ThreadItem,
  ThreadSummary,
  TurnId,
  TurnState
} from '../../../shared/contracts'
import type { StreamStatus } from '../api/events-api'

export type { ChildCompletionNotice, LoadStatus } from '../../../shared/contracts'

/** One runtime `error` event surfaced inline in the stream. */
export type ErrorLine = {
  seq: number
  timestamp: string
  code: ArkErrorCode | 'unknown'
  message: string
}

/**
 * Per-thread usage projection, folded from the runtime's authoritative `usage`
 * events. `lastPromptTokens` is the newest sample's prompt size — the closest
 * client-side proxy for how full the context is; the limit itself comes from
 * runtime tuning (`context_limit_tokens`).
 */
export type UsageAggregate = {
  samples: number
  promptTokens: number
  completionTokens: number
  totalTokens: number
  costUsd: number | null
  lastPromptTokens: number | null
}

/** Latest `compaction_completed` notice; diagnostics for the context meter. */
export type CompactionNotice = {
  removedItems: number
  checkpointChars: number
  seq: number
}

/**
 * Gates are the "few control events" the renderer is still allowed to read
 * (design §7.10.3): they carry the id the user must answer with.
 *
 * Derived from the generated contract views so a field added on the Rust side
 * fails `tsc` here instead of being silently dropped.
 */
export type PendingApproval = PendingApprovalView & {
  status: 'pending' | 'approved' | 'rejected'
}

export type PendingUserInput = PendingUserInputView & {
  status: 'pending' | 'answered'
  answer?: string
}

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
  /**
   * Advances for every `thread_rewound` notification. The writer truncated the
   * durable log and never reuses sequence numbers, so the local projection is
   * stale by construction and must be rebuilt from the detail watermark.
   */
  rewindRevision: number
  /** `rewind_from_seq` of the newest notification; diagnostics only. */
  rewoundFromSeq: number | null
  /** Aggregated `usage` events; null until the first sample or thread seed. */
  usage: UsageAggregate | null
  /** Newest compaction notice, cleared on rewind with the rest of the projection. */
  lastCompaction: CompactionNotice | null
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
