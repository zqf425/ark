/**
 * Pure `RuntimeEvent -> state` reducer.
 *
 * The semantic mapping already happened in Rust (design §7.10.3). What is left
 * here is bookkeeping the server cannot do for us: replacing snapshots, appending
 * deltas, tracking `seq`, and recording gate requests that must be answered.
 *
 * Folding happens on a copy-on-write draft (see `items-draft.ts`) so a single
 * streamed token does not replace every channel of the snapshot. Channels and
 * item objects that a fold does not touch keep their previous identity, which is
 * what allows the canvas to memoize its projections.
 */
import type { ItemId, ItemKind, RuntimeEvent, ThreadItem, ThreadRuntimeSnapshot, TurnId, TurnState } from '../../../shared/contracts'
import {
  commitDraft,
  draftApprovals,
  draftChildRunStatuses,
  draftItemOrder,
  draftItems,
  draftOf,
  draftPlan,
  draftPendingGateCounts,
  draftRoundIndex,
  draftTurnLastEventAt,
  draftTurnState,
  draftUserInputs,
  sameItemContent,
  type ItemsDraft
} from './items-draft'
import { appendUsageTail, foldUsage, usageTailHasSeq } from './usage'
import { appendRound } from './rounds'
import { projectToolCall, projectUserInput } from './item-projection'
import type { CompactionNotice, ItemsSnapshot, PendingApproval, PendingUserInput } from './types'

export function emptyItemsState(): ItemsSnapshot {
  return {
    itemsById: {},
    itemOrder: [],
    turnState: {},
    activeTurnId: null,
    lastSeq: 0,
    approvals: {},
    userInputs: {},
    childRunRevision: 0,
    childRunActiveCount: 0,
    childRunStatuses: {},
    workbenchRevision: 0,
    rewindRevision: 0,
    rewoundFromSeq: null,
    usage: null,
    usageTurns: null,
    usageRequest: null,
    usageTail: null,
    usageSeedSeq: 0,
    roundWindow: null,
    lastCompaction: null,
    rounds: [],
    roundIndex: {},
    pendingGateCounts: {},
    turnLastEventAt: {},
    changedItemIds: [],
    itemOrderChanged: false,
    liveItemCount: 0,
    plan: { todos: [] }
  }
}

/** Deltas name their item kind by event; no content inspection is involved. */
const DELTA_ITEM_KIND: Partial<Record<RuntimeEvent['kind'], ItemKind>> = {
  assistant_text_delta: 'assistant_message',
  assistant_reasoning_delta: 'reasoning'
}

function foldToolCall(
  draft: ItemsDraft,
  event: Extract<RuntimeEvent, { kind: 'tool_call_started' | 'tool_call_finished' }>
): void {
  const id = event.item_id!
  upsertItem(draft, projectToolCall(event, currentItem(draft, id)))
}

/**
 * Streaming windows shorter than this are too noisy for a tok/s readout: a
 * near-instant round would otherwise print absurd spikes. Rounds below the
 * threshold simply show no speed.
 */
const MIN_STREAM_WINDOW_SECONDS = 0.3

/** Epoch ms of a runtime event timestamp (ISO 8601 from the wire). */
function eventMs(timestamp: string): number {
  return Date.parse(timestamp)
}

function currentTurnState(draft: ItemsDraft, turnId: TurnId): TurnState | undefined {
  return draft.turnState ? draft.turnState[turnId] : draft.base.turnState[turnId]
}

function ensureRound(draft: ItemsDraft, turnId: TurnId): void {
  const position = (draft.roundIndex ?? draft.base.roundIndex)[turnId]
  if (position !== undefined && draft.rounds[position]?.turnId === turnId) return
  draftRoundIndex(draft)[turnId] = draft.rounds.length
  draft.rounds = [...draft.rounds, { turnId, durationMs: 0, activeMs: 0 }]
}

function turnHasPendingGate(draft: ItemsDraft, turnId: TurnId): boolean {
  return ((draft.pendingGateCounts ?? draft.base.pendingGateCounts)[turnId] ?? 0) > 0
}

function changePendingGateCount(draft: ItemsDraft, turnId: TurnId, delta: 1 | -1): void {
  const counts = draftPendingGateCounts(draft)
  const next = (counts[turnId] ?? 0) + delta
  if (next > 0) counts[turnId] = next
  else delete counts[turnId]
}

/** Bank active wall time before advancing the anchor; gate waits never accrue. */
function advanceActiveClock(draft: ItemsDraft, turnId: TurnId, timestamp: string): void {
  const at = eventMs(timestamp)
  if (!Number.isFinite(at)) return
  const state = currentTurnState(draft, turnId)
  if (state === 'running' || state === 'aborting') {
    ensureRound(draft, turnId)
    const anchors = draftTurnLastEventAt(draft)
    const previous = anchors[turnId]
    if (previous != null && at > previous && !turnHasPendingGate(draft, turnId)) {
      const position = (draft.roundIndex ?? draft.base.roundIndex)[turnId]!
      const round = draft.rounds[position]!
      draft.rounds = draft.rounds.slice()
      draft.rounds[position] = { ...round, activeMs: (round.activeMs ?? 0) + at - previous }
    }
    if (previous == null || at >= previous) anchors[turnId] = at
    return
  }
  // The anchor describes the newest event even before a turn-state row exists;
  // a mid-turn mount can still establish a live clock from it.
  const anchors = draftTurnLastEventAt(draft)
  const previous = anchors[turnId]
  if (previous == null || at >= previous) anchors[turnId] = at
}

/**
 * Track the streaming window of the in-flight model round: from the round's
 * first delta to its latest one. The next `usage` event pairs its
 * provider-reported completion tokens with this window to derive tok/s.
 *
 * `startMs` falls back to the round boundary (turn start or the previous
 * round's `usage`), so a round without visible deltas still yields a sane
 * duration estimate for the per-round fold.
 */
function trackDeltaWindow(draft: ItemsDraft, timestamp: string): void {
  const ms = eventMs(timestamp)
  draft.roundWindow = { startMs: draft.roundWindow?.startMs ?? ms, lastMs: ms }
}

/** A round boundary invalidates any in-flight streaming window. */
function resetDeltaWindow(draft: ItemsDraft): void {
  draft.roundWindow = null
}

/** Runtime events that change one of the server-side workbench projections. */
const WORKBENCH_EVENT_KINDS: ReadonlySet<RuntimeEvent['kind']> = new Set([
  'goal_updated',
  'goal_cleared',
  'todos_updated',
  'todos_cleared',
  'tool_call_started',
  'tool_call_finished'
])

/** Draft-first read: a copied channel is authoritative, otherwise use the base. */
function currentItem(draft: ItemsDraft, itemId: ItemId): ThreadItem | undefined {
  return draft.itemsById ? draft.itemsById[itemId] : draft.base.itemsById[itemId]
}

const isLiveItem = (item: ThreadItem): boolean =>
  item.status === 'pending' || item.status === 'streaming'

function noteItemWritten(draft: ItemsDraft, item: ThreadItem, previous?: ThreadItem): void {
  if (previous && isLiveItem(previous) !== isLiveItem(item)) {
    draft.liveItemCount += isLiveItem(item) ? 1 : -1
  }
  draft.changedItemIds.push(item.item_id)
}

/**
 * Snapshot events are authoritative, but a snapshot may be partial (status-only
 * `item_updated`). Field-level merge keeps accumulated text in that case.
 *
 * A re-sent item whose content already matches keeps the stored object: the
 * snapshot is authoritative in content, not in object identity.
 */
function upsertItem(draft: ItemsDraft, item: ThreadItem): void {
  const existing = currentItem(draft, item.item_id)
  if (!existing) {
    draftItems(draft)[item.item_id] = item
    draftItemOrder(draft).push(item.item_id)
    if (isLiveItem(item)) draft.liveItemCount += 1
    noteItemWritten(draft, item)
    return
  }

  const merged: ThreadItem = {
    ...existing,
    ...item,
    text: item.text ?? existing.text,
    tool: item.tool ?? existing.tool,
    detail: item.detail ?? existing.detail,
    attachments: item.attachments ?? existing.attachments,
    model: item.model ?? existing.model
  }
  if (sameItemContent(existing, merged)) return
  draftItems(draft)[item.item_id] = merged
  noteItemWritten(draft, merged, existing)
}

function appendDelta(
  draft: ItemsDraft,
  event: Extract<RuntimeEvent, { kind: 'assistant_text_delta' | 'assistant_reasoning_delta' }>
): void {
  const existing = currentItem(draft, event.item_id)
  if (existing) {
    const updated: ThreadItem = {
      ...existing,
      text: (existing.text ?? '') + event.delta,
      status: existing.status === 'pending' ? 'streaming' : existing.status
    }
    draftItems(draft)[event.item_id] = updated
    noteItemWritten(draft, updated, existing)
    return
  }

  // The runtime normally creates the item first; if a delta wins the race we
  // materialise a streaming row so no token is silently dropped.
  const kind = DELTA_ITEM_KIND[event.kind] ?? 'assistant_message'
  const created: ThreadItem = {
    item_id: event.item_id,
    turn_id: event.turn_id,
    kind,
    status: 'streaming',
    created_at: event.timestamp,
    text: event.delta
  }
  draftItems(draft)[event.item_id] = created
  draftItemOrder(draft).push(event.item_id)
  draft.liveItemCount += 1
  noteItemWritten(draft, created)
}

function setTurn(draft: ItemsDraft, turnId: TurnId, state: TurnState): void {
  ensureRound(draft, turnId)
  draftTurnState(draft)[turnId] = state
  const terminal = state === 'completed' || state === 'failed' || state === 'aborted'
  if (terminal && draft.activeTurnId === turnId) draft.activeTurnId = null
}

/** Statuses after which a child run no longer counts as active for the badge. */
const TERMINAL_CHILD_STATUSES = new Set(['completed', 'failed', 'aborted', 'ended'])
const childStatusActive = (status: string | undefined): boolean =>
  status !== undefined && (status === 'queued' || status === 'running' || !TERMINAL_CHILD_STATUSES.has(status))

function noteChildStatus(draft: ItemsDraft, childId: string, status: string): void {
  const statuses = draftChildRunStatuses(draft)
  const wasActive = childStatusActive(statuses[childId])
  statuses[childId] = status
  if (wasActive !== childStatusActive(status)) {
    draft.childRunActiveCount += childStatusActive(status) ? 1 : -1
  }
}

/** Seed history from `GET /v1/threads/{id}`. */
export function seedFromItems(
  items: ThreadItem[],
  lastSeq: number,
  runtime: ThreadRuntimeSnapshot,
  snapshotAt: number = Date.now(),
  previous?: ItemsSnapshot
): ItemsSnapshot {
  const draft = draftOf(emptyItemsState())
  draft.lastSeq = lastSeq
  for (const item of items) upsertItem(draft, item)
  if (runtime.active_turn) {
    const turnId = runtime.active_turn.turn_id
    const continuing = previous?.activeTurnId === turnId && previous.lastSeq <= lastSeq
    draft.activeTurnId = turnId
    // An active registry entry can only be running or aborting. Keep an
    // unknown future wire string actionable as running rather than inventing a
    // terminal state in the browser.
    draftTurnState(draft)[turnId] =
      runtime.active_turn.state === 'aborting' ? 'aborting' : 'running'
    if (continuing && previous) {
      // A mid-turn detail reload is the same clock, not a new mount. Reusing
      // these channels keeps banked active time, the newest event anchor and
      // the in-flight model window stable across the authoritative item reload.
      draft.rounds = previous.rounds
      draft.roundIndex = previous.roundIndex
      draft.turnLastEventAt = previous.turnLastEventAt
      draft.roundWindow = previous.roundWindow
      ensureRound(draft, turnId)
    } else {
      ensureRound(draft, turnId)
      if (Number.isFinite(snapshotAt)) {
        // The turn started before this detail was read. Continuing from that
        // timestamp would bank the whole elapsed turn again and stretch the first
        // post-mount streaming round across history the user already saw.
        draftTurnLastEventAt(draft)[turnId] = snapshotAt
        draft.roundWindow = null
      }
    }
  }
  for (const approval of runtime.pending_approvals) {
    draftApprovals(draft)[approval.approval_id] = { ...approval, status: 'pending' }
    changePendingGateCount(draft, approval.turn_id, 1)
  }
  for (const request of runtime.pending_user_inputs) {
    draftUserInputs(draft)[request.request_id] = { ...request, status: 'pending' }
    changePendingGateCount(draft, request.turn_id, 1)
  }
  return commitDraft(draft)
}

/**
 * Fold one SSE event. Unknown kinds only advance `seq`, which keeps the
 * reconnect watermark correct even for events the UI does not render.
 */
export function applyEvent(state: ItemsSnapshot, event: RuntimeEvent): ItemsSnapshot {
  // A frame at or below the seeded watermark is history the authoritative
  // detail snapshot already contains. Folding it again appended the same text
  // delta a second time and double-counted usage — the duplicated-message
  // symptom. Drop it without allocating a new state object. A usage frame may
  // arrive below a newer frame while its HTTP owner is in flight; retain that
  // unseen suffix once, in the tail, so response coverage can replay it by seq.
  const request = state.usageRequest
  const latePendingUsage =
    event.kind === 'usage' &&
    request !== null &&
    event.seq <= state.lastSeq &&
    event.seq > request.seq &&
    event.seq > state.usageSeedSeq &&
    !usageTailHasSeq(state.usageTail, event.seq)
  if (event.seq <= state.lastSeq && !latePendingUsage) return state
  const draft = draftOf(state)
  if (event.seq > draft.lastSeq) draft.lastSeq = event.seq
  // Workbench panels subscribe to this revision instead of `lastSeq`: the
  // latter advances on every streamed token, which made each token trigger
  // ~6 full HTTP reloads. Only these events change a server projection.
  if (WORKBENCH_EVENT_KINDS.has(event.kind)) draft.workbenchRevision += 1
  // Bank active time before moving the anchor. Gate-resolution frames resume
  // from their own timestamp; thread-scoped events carry no turn clock.
  if ('turn_id' in event) advanceActiveClock(draft, event.turn_id, event.timestamp)

  switch (event.kind) {
    case 'thread_created':
    case 'thread_updated':
      // The stream layer folds these into rail rows; the transcript still
      // advances its watermark so replay deduplication remains monotonic.
      return commitDraft(draft)

    case 'item_created':
    case 'item_updated':
    case 'item_completed':
      upsertItem(draft, event.item)
      return commitDraft(draft)

    case 'assistant_text_delta':
    case 'assistant_reasoning_delta':
      appendDelta(draft, event)
      trackDeltaWindow(draft, event.timestamp)
      return commitDraft(draft)

    case 'tool_call_started':
    case 'tool_call_finished':
      // The realtime channel carries bare tool events (the server folds them
      // into the durable projection only); without this fold the live canvas
      // misses every tool row of an in-flight turn.
      foldToolCall(draft, event)
      return commitDraft(draft)

    case 'turn_started': {
      draft.activeTurnId = event.turn_id
      setTurn(draft, event.turn_id, 'running')
      advanceActiveClock(draft, event.turn_id, event.timestamp)
      // Seed the round boundary so a first round without visible deltas
      // still measures from the turn start when its `usage` lands.
      draft.roundWindow = { startMs: eventMs(event.timestamp), lastMs: eventMs(event.timestamp) }
      return commitDraft(draft)
    }

    case 'turn_completed':
      setTurn(draft, event.turn_id, 'completed')
      resetDeltaWindow(draft)
      return commitDraft(draft)

    case 'turn_failed': {
      // The failure reason renders inline at the interruption point. The
      // server-side projector folds `turn_failed` into a durable `error`
      // item, but that projection only lives in `items.jsonl` — the live
      // SSE channel carries the bare `turn_failed` event. Materialize the
      // same row here using the item id supplied by Rust so the live row and
      // the replayed row are one item.
      setTurn(draft, event.turn_id, 'failed')
      resetDeltaWindow(draft)
      upsertItem(draft, {
        item_id: event.item_id!,
        turn_id: event.turn_id,
        kind: 'error',
        status: 'failed',
        created_at: event.timestamp,
        text: event.message
      })
      return commitDraft(draft)
    }

    case 'thread_rewound': {
      // The writer physically truncated `events.jsonl` and appended this
      // notification at a fresh, never-reused seq (docs/modules/ark-store.md).
      // Every row, turn state and gate projected from the discarded suffix is
      // gone, so drop the projection rather than leave deleted turns on screen;
      // `rewindRevision` tells the stream layer to rebuild from the detail
      // watermark. `lastSeq` is kept: it is the notification's own seq, which is
      // already ahead of anything the discarded suffix ever carried.
      const rebuilt = emptyItemsState()
      rebuilt.lastSeq = draft.lastSeq
      rebuilt.rewindRevision = draft.rewindRevision + 1
      rebuilt.rewoundFromSeq = event.rewind_from_seq
      // Child runs and every workbench projection may have been discarded with
      // the suffix; let those panels refetch. Revisions stay monotonic so a
      // rebuilt projection never looks older than the one it replaced.
      rebuilt.childRunRevision = draft.childRunRevision + 1
      rebuilt.childRunActiveCount = 0
      rebuilt.workbenchRevision = draft.workbenchRevision + 1
      return rebuilt
    }

    case 'turn_aborted':
      setTurn(draft, event.turn_id, 'aborted')
      resetDeltaWindow(draft)
      return commitDraft(draft)

    case 'transient_retry':
    case 'error':
      // Diagnostic events without a durable item: transient retries and
      // stream-level diagnostics surface through logs/toasts, never as a
      // footer pinned under the stream.
      return commitDraft(draft)

    case 'approval_requested': {
      const existing = draft.approvals
        ? draft.approvals[event.approval_id]
        : draft.base.approvals[event.approval_id]
      const approval: PendingApproval = {
        approval_id: event.approval_id,
        turn_id: event.turn_id,
        tool: event.tool,
        summary: event.summary,
        status: 'pending',
        requested_at: event.timestamp
      }
      draftApprovals(draft)[event.approval_id] = approval
      if (existing?.status !== 'pending') changePendingGateCount(draft, event.turn_id, 1)
      return commitDraft(draft)
    }

    case 'approval_resolved': {
      const existing = draft.approvals
        ? draft.approvals[event.approval_id]
        : draft.base.approvals[event.approval_id]
      if (existing) {
        if (existing.status === 'pending') changePendingGateCount(draft, existing.turn_id, -1)
        draftApprovals(draft)[event.approval_id] = {
          ...existing,
          status: event.approved ? 'approved' : 'rejected'
        }
      }
      return commitDraft(draft)
    }

    case 'user_input_requested': {
      const existing = draft.userInputs
        ? draft.userInputs[event.request_id]
        : draft.base.userInputs[event.request_id]
      const request: PendingUserInput = {
        request_id: event.request_id,
        turn_id: event.turn_id,
        prompt: event.prompt,
        status: 'pending',
        requested_at: event.timestamp
      }
      draftUserInputs(draft)[event.request_id] = request
      if (existing?.status !== 'pending') changePendingGateCount(draft, event.turn_id, 1)
      upsertItem(draft, projectUserInput(event))
      return commitDraft(draft)
    }

    case 'user_input_resolved': {
      const existing = draft.userInputs
        ? draft.userInputs[event.request_id]
        : draft.base.userInputs[event.request_id]
      if (existing) {
        if (existing.status === 'pending') changePendingGateCount(draft, existing.turn_id, -1)
        draftUserInputs(draft)[event.request_id] = { ...existing, status: 'answered', answer: event.answer }
      }
      const id = event.item_id!
      upsertItem(draft, projectUserInput(event, currentItem(draft, id)))
      return commitDraft(draft)
    }

    case 'child_completed':
      // The durable summary lives in the subagent panel; only lifecycle counts
      // belong in the transcript projection.
      draft.childRunRevision += 1
      noteChildStatus(draft, event.child_id, event.status)
      return commitDraft(draft)

    case 'child_activity': {
      draft.childRunRevision += 1
      noteChildStatus(draft, event.child_id, event.status)
      return commitDraft(draft)
    }

    case 'usage': {
      // tok/s pairs the round's provider-reported completion tokens with the
      // window measured from its own deltas; both are dropped afterwards so a
      // round without visible streaming shows no speed instead of a stale one.
      const window = draft.roundWindow
      const seconds = window ? (window.lastMs - window.startMs) / 1000 : 0
      const tokensPerSec =
        window && seconds >= MIN_STREAM_WINDOW_SECONDS ? event.usage.completion_tokens / seconds : null
      draft.usage = foldUsage(draft.usage, event.usage, tokensPerSec)
      if (draft.usageRequest) {
        draft.usageTail = appendUsageTail(draft.usageTail, {
          seq: event.seq,
          usage: event.usage,
          roundTokensPerSec: tokensPerSec
        })
      }
      // The same event closes one model round: record its duration (the
      // provider-measured value when present, the client window otherwise)
      // so the turn total covers rounds even before the next HTTP refresh.
      const measuredMs =
        event.usage.duration_ms != null && event.usage.duration_ms > 0
          ? event.usage.duration_ms
          : window && window.lastMs >= window.startMs
            ? window.lastMs - window.startMs
            : 0
      draft.rounds = appendRound(
        draft.rounds,
        { turnId: event.turn_id, durationMs: measuredMs },
        draftRoundIndex(draft)
      )
      resetDeltaWindow(draft)
      return commitDraft(draft)
    }

    case 'goal_updated': {
      const plan = draftPlan(draft)
      plan.goal = event.goal
      return commitDraft(draft)
    }

    case 'goal_cleared': {
      const plan = draftPlan(draft)
      delete plan.goal
      return commitDraft(draft)
    }

    case 'todos_updated': {
      const plan = draftPlan(draft)
      plan.todos = event.todos.map((todo) => ({ ...todo }))
      return commitDraft(draft)
    }

    case 'todos_cleared': {
      draftPlan(draft).todos = []
      return commitDraft(draft)
    }

    case 'compaction_completed': {
      const notice: CompactionNotice = {
        removedItems: event.removed_items,
        checkpointChars: event.checkpoint_chars,
        seq: event.seq
      }
      draft.lastCompaction = notice
      return commitDraft(draft)
    }

    default: {
      // The wire union is Rust-owned and may grow ahead of this renderer; an
      // unrecognized event is dropped from the projection, but never silently:
      // a version skew should be visible in the console during development.
      if (import.meta.env.DEV) {
        console.warn(`[ark] unhandled runtime event kind: ${String(event.kind)}`)
      }
      return commitDraft(draft)
    }
  }
}

/** Client-side echo of a locally resolved gate, before the server confirms. */
export function markApprovalLocally(
  state: ItemsSnapshot,
  approvalId: string,
  approved: boolean
): ItemsSnapshot {
  const existing = state.approvals[approvalId]
  if (!existing || existing.status !== 'pending') return state
  const draft = draftOf(state)
  changePendingGateCount(draft, existing.turn_id, -1)
  draftApprovals(draft)[approvalId] = { ...existing, status: approved ? 'approved' : 'rejected' }
  return commitDraft(draft)
}

export function markUserInputLocally(state: ItemsSnapshot, requestId: string, answer: string): ItemsSnapshot {
  const existing = state.userInputs[requestId]
  if (!existing || existing.status !== 'pending') return state
  const draft = draftOf(state)
  changePendingGateCount(draft, existing.turn_id, -1)
  draftUserInputs(draft)[requestId] = { ...existing, status: 'answered', answer }
  return commitDraft(draft)
}
