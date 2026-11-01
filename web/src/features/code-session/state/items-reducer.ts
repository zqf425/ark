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
import { translateNow } from '../../../shared/i18n/current'
import {
  commitDraft,
  draftApprovals,
  draftChildCompletions,
  draftErrors,
  draftItemOrder,
  draftItems,
  draftOf,
  draftTurnState,
  draftUserInputs,
  sameItemContent,
  type ItemsDraft
} from './items-draft'
import type { ChildCompletionNotice, ErrorLine, ItemsSnapshot, PendingApproval, PendingUserInput } from './types'

export function emptyItemsState(): ItemsSnapshot {
  return {
    itemsById: {},
    itemOrder: [],
    turnState: {},
    activeTurnId: null,
    lastSeq: 0,
    errors: [],
    approvals: {},
    userInputs: {},
    childCompletions: {},
    childRunRevision: 0,
    rewindRevision: 0,
    rewoundFromSeq: null
  }
}

/** Deltas name their item kind by event; no content inspection is involved. */
const DELTA_ITEM_KIND: Partial<Record<RuntimeEvent['kind'], ItemKind>> = {
  assistant_text_delta: 'assistant_message',
  assistant_reasoning_delta: 'reasoning'
}

const MAX_ERROR_LINES = 50
const MAX_CHILD_COMPLETIONS = 50

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
    detail: item.detail ?? existing.detail
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

function pushError(draft: ItemsDraft, line: ErrorLine): void {
  draft.errors = [...draftErrors(draft), line].slice(-MAX_ERROR_LINES)
}

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
  const draft = draftOf(state)
  if (event.seq > draft.lastSeq) draft.lastSeq = event.seq

  switch (event.kind) {
    case 'item_created':
    case 'item_updated':
    case 'item_completed':
      upsertItem(draft, event.item)
      return commitDraft(draft)

    case 'assistant_text_delta':
    case 'assistant_reasoning_delta':
      appendDelta(draft, event)
      return commitDraft(draft)

    case 'turn_started':
      draft.activeTurnId = event.turn_id
      setTurn(draft, event.turn_id, 'running')
      return commitDraft(draft)

    case 'turn_completed':
      setTurn(draft, event.turn_id, 'completed')
      return commitDraft(draft)

    case 'turn_failed':
      setTurn(draft, event.turn_id, 'failed')
      pushError(draft, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: 'internal_error',
        message: event.message
      })
      return commitDraft(draft)

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
      // Child runs may have been discarded with the suffix; let the panel refetch.
      rebuilt.childRunRevision = draft.childRunRevision + 1
      return rebuilt
    }

    case 'turn_aborted':
      setTurn(draft, event.turn_id, 'aborted')
      return commitDraft(draft)

    case 'transient_retry':
      pushError(draft, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: 'model_request_failed',
        message: translateNow('canvas.retryAttempt', { attempt: event.attempt, message: event.message })
      })
      return commitDraft(draft)

    case 'error':
      pushError(draft, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: event.code,
        message: event.message
      })
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

    case 'child_activity':
      draft.childRunRevision += 1
      return commitDraft(draft)

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
