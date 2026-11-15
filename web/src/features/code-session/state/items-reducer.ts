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
  draftChildCompletions,
  draftItemOrder,
  draftItems,
  draftOf,
  draftTurnState,
  draftUserInputs,
  sameItemContent,
  type ItemsDraft
} from './items-draft'
import { foldUsage } from './usage'
import type { ChildCompletionNotice, CompactionNotice, ItemsSnapshot, PendingApproval, PendingUserInput } from './types'

export function emptyItemsState(): ItemsSnapshot {
  return {
    itemsById: {},
    itemOrder: [],
    turnState: {},
    activeTurnId: null,
    lastSeq: 0,
    approvals: {},
    userInputs: {},
    childCompletions: {},
    childRunRevision: 0,
    childRunActiveCount: 0,
    workbenchRevision: 0,
    rewindRevision: 0,
    rewoundFromSeq: null,
    usage: null,
    roundWindow: null,
    lastCompaction: null
  }
}

/** Deltas name their item kind by event; no content inspection is involved. */
const DELTA_ITEM_KIND: Partial<Record<RuntimeEvent['kind'], ItemKind>> = {
  assistant_text_delta: 'assistant_message',
  assistant_reasoning_delta: 'reasoning'
}

const MAX_CHILD_COMPLETIONS = 50

/**
 * Deterministic `ItemId` for one turn's terminal failure, mirroring the Rust
 * `keyed_item_id("turn_failed:{turn_id}")` (ark-store domain/items.rs): FNV-1a
 * over the namespace string, hi/lo halves seeded with the offset basis pairs,
 * packed into a UUID with the custom-variant bits forced. The server stores the
 * error item under this id in `items.jsonl`, so the live-folded row and the
 * replayed row are the same item and a seed merge never duplicates it.
 */
const FNV_BASIS_HI = 0xcbf29ce484222325n
const FNV_BASIS_LO = 0x84222325cbf29ce4n
const FNV_PRIME = 0x100000001b3n

function fnv1a(bytes: Uint8Array, seed: bigint): bigint {
  let hash = seed
  for (const byte of bytes) {
    hash ^= BigInt(byte)
    hash = (hash * FNV_PRIME) & 0xffffffffffffffffn
  }
  return hash
}

function turnFailedItemId(turnId: TurnId): ItemId {
  const namespace = new TextEncoder().encode(`turn_failed:${turnId}`)
  const hi = fnv1a(namespace, FNV_BASIS_HI)
  const lo = fnv1a(namespace, FNV_BASIS_LO)
  const view = new DataView(new ArrayBuffer(16))
  view.setBigUint64(0, hi)
  view.setBigUint64(8, lo)
  const bytes = new Uint8Array(view.buffer)
  bytes[6] = (bytes[6]! & 0x0f) | 0x80
  bytes[8] = (bytes[8]! & 0x3f) | 0x80
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}` as ItemId
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

/**
 * Track the streaming window of the in-flight model round: from the round's
 * first delta to its latest one. The next `usage` event pairs its
 * provider-reported completion tokens with this window to derive tok/s.
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

function currentChildCompletion(draft: ItemsDraft, id: string): ChildCompletionNotice | undefined {
  return draft.childCompletions ? draft.childCompletions[id] : draft.base.childCompletions[id]
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
    return
  }

  const merged: ThreadItem = {
    ...existing,
    ...item,
    text: item.text ?? existing.text,
    tool: item.tool ?? existing.tool,
    detail: item.detail ?? existing.detail,
    attachments: item.attachments ?? existing.attachments
  }
  if (sameItemContent(existing, merged)) return
  draftItems(draft)[item.item_id] = merged
}

function appendDelta(
  draft: ItemsDraft,
  event: Extract<RuntimeEvent, { kind: 'assistant_text_delta' | 'assistant_reasoning_delta' }>
): void {
  const existing = currentItem(draft, event.item_id)
  if (existing) {
    draftItems(draft)[event.item_id] = {
      ...existing,
      text: (existing.text ?? '') + event.delta,
      status: existing.status === 'pending' ? 'streaming' : existing.status
    }
    return
  }

  // The runtime normally creates the item first; if a delta wins the race we
  // materialise a streaming row so no token is silently dropped.
  const kind = DELTA_ITEM_KIND[event.kind] ?? 'assistant_message'
  draftItems(draft)[event.item_id] = {
    item_id: event.item_id,
    turn_id: event.turn_id,
    kind,
    status: 'streaming',
    created_at: event.timestamp,
    text: event.delta
  }
  draftItemOrder(draft).push(event.item_id)
}

function setTurn(draft: ItemsDraft, turnId: TurnId, state: TurnState): void {
  draftTurnState(draft)[turnId] = state
  const terminal = state === 'completed' || state === 'failed' || state === 'aborted'
  if (terminal && draft.activeTurnId === turnId) draft.activeTurnId = null
}

/** Statuses after which a child run no longer counts as active for the badge. */
const TERMINAL_CHILD_STATUSES = new Set(['completed', 'failed', 'aborted', 'ended'])

/** Keep UI-only completion cards bounded even if a stream is unusually long. */
function recordChildCompletion(draft: ItemsDraft, notice: ChildCompletionNotice): void {
  if (currentChildCompletion(draft, notice.notification_id)) return
  const completions = draftChildCompletions(draft)
  completions[notice.notification_id] = notice
  const notificationIds = Object.keys(completions)
  if (notificationIds.length <= MAX_CHILD_COMPLETIONS) return

  const oldestId = notificationIds.reduce((oldest, id) => {
    const candidate = completions[id]
    const current = completions[oldest]
    return candidate && current && candidate.seq < current.seq ? id : oldest
  })
  delete completions[oldestId]
}

/** Seed history from `GET /v1/threads/{id}`. */
export function seedFromItems(
  items: ThreadItem[],
  lastSeq: number,
  childCompletions: ChildCompletionNotice[],
  runtime: ThreadRuntimeSnapshot
): ItemsSnapshot {
  const draft = draftOf(emptyItemsState())
  draft.lastSeq = lastSeq
  for (const item of items) upsertItem(draft, item)
  for (const notice of childCompletions) recordChildCompletion(draft, notice)
  if (runtime.active_turn) {
    draft.activeTurnId = runtime.active_turn.turn_id
    // An active registry entry can only be running or aborting. Keep an
    // unknown future wire string actionable as running rather than inventing a
    // terminal state in the browser.
    draftTurnState(draft)[runtime.active_turn.turn_id] =
      runtime.active_turn.state === 'aborting' ? 'aborting' : 'running'
  }
  for (const approval of runtime.pending_approvals) {
    draftApprovals(draft)[approval.approval_id] = { ...approval, status: 'pending' }
  }
  for (const request of runtime.pending_user_inputs) {
    draftUserInputs(draft)[request.request_id] = { ...request, status: 'pending' }
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
  // symptom. Drop it without allocating a new state object.
  if (event.seq <= state.lastSeq) return state
  const draft = draftOf(state)
  if (event.seq > draft.lastSeq) draft.lastSeq = event.seq
  // Workbench panels subscribe to this revision instead of `lastSeq`: the
  // latter advances on every streamed token, which made each token trigger
  // ~6 full HTTP reloads. Only these events change a server projection.
  if (WORKBENCH_EVENT_KINDS.has(event.kind)) draft.workbenchRevision += 1

  switch (event.kind) {
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

    case 'turn_started':
      draft.activeTurnId = event.turn_id
      setTurn(draft, event.turn_id, 'running')
      resetDeltaWindow(draft)
      return commitDraft(draft)

    case 'turn_completed':
      setTurn(draft, event.turn_id, 'completed')
      resetDeltaWindow(draft)
      return commitDraft(draft)

    case 'turn_failed': {
      // The failure reason renders inline at the interruption point. The
      // server-side projector folds `turn_failed` into a durable `error`
      // item, but that projection only lives in `items.jsonl` — the live
      // SSE channel carries the bare `turn_failed` event. Materialize the
      // same row here with the server's deterministic id
      // (`turn_failed:{turn_id}`, mirrored from the Rust projector) so the
      // live row and the replayed row are one item: a later seed merge
      // keeps one row instead of duplicating it.
      setTurn(draft, event.turn_id, 'failed')
      resetDeltaWindow(draft)
      upsertItem(draft, {
        item_id: turnFailedItemId(event.turn_id),
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
      const approval: PendingApproval = {
        approval_id: event.approval_id,
        turn_id: event.turn_id,
        tool: event.tool,
        summary: event.summary,
        status: 'pending',
        requested_at: event.timestamp
      }
      draftApprovals(draft)[event.approval_id] = approval
      return commitDraft(draft)
    }

    case 'approval_resolved': {
      const existing = draft.base.approvals[event.approval_id]
      if (existing) {
        draftApprovals(draft)[event.approval_id] = {
          ...existing,
          status: event.approved ? 'approved' : 'rejected'
        }
      }
      return commitDraft(draft)
    }

    case 'user_input_requested': {
      const request: PendingUserInput = {
        request_id: event.request_id,
        turn_id: event.turn_id,
        prompt: event.prompt,
        status: 'pending',
        requested_at: event.timestamp
      }
      draftUserInputs(draft)[event.request_id] = request
      return commitDraft(draft)
    }

    case 'user_input_resolved': {
      const existing = draft.base.userInputs[event.request_id]
      if (existing) {
        draftUserInputs(draft)[event.request_id] = { ...existing, status: 'answered', answer: event.answer }
      }
      return commitDraft(draft)
    }

    case 'child_completed': {
      const notice: ChildCompletionNotice = {
        notification_id: event.notification_id,
        child_id: event.child_id,
        thread_id: event.thread_id,
        label: event.label,
        status: event.status,
        summary: event.summary,
        error: event.error,
        completed_at: event.timestamp,
        seq: event.seq
      }
      recordChildCompletion(draft, notice)
      draft.childRunRevision += 1
      return commitDraft(draft)
    }

    case 'child_activity': {
      draft.childRunRevision += 1
      // The event carries the child's own status, so the badge count tracks it
      // without a second request. An unknown future wire status counts as
      // active: a badge that over-reports beats one that hides live work.
      const active = event.status === 'queued' || event.status === 'running' || !TERMINAL_CHILD_STATUSES.has(event.status)
      draft.childRunActiveCount = Math.max(0, draft.childRunActiveCount + (active ? 1 : -1))
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
      resetDeltaWindow(draft)
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
  if (!existing) return state
  return {
    ...state,
    approvals: { ...state.approvals, [approvalId]: { ...existing, status: approved ? 'approved' : 'rejected' } }
  }
}

export function markUserInputLocally(state: ItemsSnapshot, requestId: string, answer: string): ItemsSnapshot {
  const existing = state.userInputs[requestId]
  if (!existing) return state
  return {
    ...state,
    userInputs: { ...state.userInputs, [requestId]: { ...existing, status: 'answered', answer } }
  }
}
