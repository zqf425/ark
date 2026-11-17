import type {
  LoadStatus,
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

export type { LoadStatus } from '../../../shared/contracts'

/**
 * Per-thread usage projection, folded from the runtime's authoritative `usage`
 * events. `lastPromptTokens` is the newest sample's prompt size — the closest
 * client-side proxy for how full the context is; the limit itself is the
 * runtime's derived compaction threshold (80% of the model's declared window).
 *
 * Cache sums follow the same `sum_opt` semantics as `costUsd`: a sample without
 * the counter contributes nothing and the sum stays null until one carries it.
 * The hit rate is only ever derived from the hit/miss pair, never faked from
 * `hit / prompt_tokens` — the same rule the Rust aggregate applies.
 */
export type UsageAggregate = {
  samples: number
  promptTokens: number
  completionTokens: number
  totalTokens: number
  costUsd: number | null
  lastPromptTokens: number | null
  cacheHitTokens: number | null
  cacheMissTokens: number | null
  /** Output speed of the newest model round, measured over its streaming window. */
  lastRoundTokensPerSec: number | null
}

/**
 * Streaming window of the in-flight model round: epoch ms of its first and
 * latest delta. Reducer bookkeeping that pairs the next `usage` sample's
 * provider-reported completion tokens with real streaming time to derive
 * tok/s; not display state.
 */
export type RoundWindow = {
  startMs: number
  lastMs: number
}

/**
 * Renderer fold of the authoritative per-round model timings. Seeded from
 * `GET /v1/debug/llm-rounds` and extended by live `usage` events; see
 * `rounds.ts` for the fold contract.
 */
export type RoundTiming = {
  turnId: TurnId
  durationMs: number
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
  approvals: Record<string, PendingApproval>
  userInputs: Record<string, PendingUserInput>
  /** Advances only for child lifecycle events, not every streamed token. */
  childRunRevision: number
  /**
   * Child runs the last lifecycle event said are queued/running. UI-only
   * affordance for the inspector badge; the panel itself always refetches the
   * authoritative list from the API.
   */
  childRunActiveCount: number
  /**
   * Advances only for events that change a server-side workbench projection
   * (plan, changes, file, terminal). Panels subscribe to this instead of
   * `lastSeq`, which advances on every streamed token.
   */
  workbenchRevision: number
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
  /** Streaming window of the in-flight model round; null between rounds. */
  roundWindow: RoundWindow | null
  /** Per-round model-call durations (snapshot-seeded, live-extended). */
  rounds: RoundTiming[]
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
