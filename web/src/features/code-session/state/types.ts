import type {
  LoadStatus,
  ItemId,
  PendingApprovalView,
  PendingUserInputView,
  PlanView,
  ThreadId,
  ThreadItem,
  ThreadSummary,
  TurnId,
  Usage,
  TurnState
} from '../../../shared/contracts'
import type { StreamStatus } from '../api/events-api'

export type { LoadStatus, PlanView } from '../../../shared/contracts'

/**
 * Per-thread usage projection, folded from the runtime's authoritative `usage`
 * events. `lastPromptTokens` is the newest usable prompt size — the closest
 * client-side proxy for how full the context is; the limit itself is the
 * runtime's derived compaction threshold (80% of the model's declared window).
 *
 * Optional counters follow Rust `Usage::merge`: sums skip missing values, but
 * one incomplete cache pair or unmeasured duration makes the corresponding
 * aggregate permanently unknown. "Unknown" must never render as zero.
 */
export type UsageAggregate = {
  samples: number
  promptTokens: number
  completionTokens: number
  totalTokens: number
  costUsd: number | null
  /** Summed measured model-call wall time; null if any sample omitted it. */
  durationMs: number | null
  durationIncomplete: boolean
  cacheHitTokens: number | null
  cacheMissTokens: number | null
  cacheCountersIncomplete: boolean
  /** Any folded sample derived its miss from a split-modality remainder. */
  cacheHitRateApproximate: boolean
  /**
   * Thread's active span from the server fold: every turn's wall clock minus
   * user-gate waits, plus harness recovery/continuation gaps, milliseconds.
   * `null` when the runtime predates the field (render nothing, not `0秒`).
   */
  activeMs: number | null
  lastPromptTokens: number | null
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

/** One owner ticket for an in-flight thread usage snapshot. */
export type UsageRequest = {
  owner: ThreadId
  /** Store watermark captured before the HTTP request started. */
  seq: number
  /** Monotonic across thread switches and detail reloads. */
  generation: number
  /** Aggregate sample count at `seq`, used to infer response coverage. */
  baseSamples: number
}

/** Live samples retained only while their snapshot owner is in flight. */
export type UsageTailSample = {
  seq: number
  usage: Usage
  roundTokensPerSec: number | null
}

/** Persistent newest-first tail: appending one sample never copies its history. */
export type UsageTailNode = {
  sample: UsageTailSample
  next: UsageTailNode | null
}

/**
 * Renderer fold of the authoritative per-turn timings. Seeded from
 * `GET /v1/threads/{id}/usage` and extended by live `usage` events; see
 * `rounds.ts` for the fold contract.
 */
export type RoundTiming = {
  turnId: TurnId
  /** Sum of measured model-call wall time in this turn, milliseconds. */
  durationMs: number
  /**
   * Server-reported or reducer-banked active span (wall clock minus user-gate
   * waits), milliseconds; null only before a clock can be established.
   */
  activeMs: number | null
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
  /** Latest lifecycle status per child, used to count unique active children. */
  childRunStatuses: Record<string, string>
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
  /** Server-reported turn count for the current aggregate. */
  usageTurns: number | null
  /** Newest usage request; stale HTTP responses cannot write this projection. */
  usageRequest: UsageRequest | null
  /** Samples received after `usageRequest.seq`, discarded when the owner settles. */
  usageTail: UsageTailNode | null
  /** Highest request watermark incorporated by an HTTP usage seed. */
  usageSeedSeq: number
  /** Streaming window of the in-flight model round; null between rounds. */
  roundWindow: RoundWindow | null
  /** Per-turn clocks (snapshot-seeded, live-extended). */
  rounds: RoundTiming[]
  /** O(1) turn-id to row lookup; maintained alongside `rounds`. */
  roundIndex: Record<TurnId, number>
  /** Number of unresolved gates per turn; avoids scanning approval history. */
  pendingGateCounts: Record<TurnId, number>
  /**
   * Newest event timestamp (epoch ms) the renderer saw per turn. The live
   * active-clock tick extends a running turn's server-reported span from this
   * anchor; terminal turns never read it.
   */
  turnLastEventAt: Record<TurnId, number>
  /** Item ids written by the newest delta; empty for control-only events. */
  changedItemIds: ItemId[]
  /** Whether the newest delta inserted rows and invalidated grouping order. */
  itemOrderChanged: boolean
  /** Constant-time transcript-liveness signal maintained by the reducer. */
  liveItemCount: number
  /** Streamed plan projection; server refetches remain authoritative. */
  plan: PlanView
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
