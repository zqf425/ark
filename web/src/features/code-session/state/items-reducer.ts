/**
 * Pure `RuntimeEvent -> state` reducer.
 *
 * The semantic mapping already happened in Rust (design §7.10.3). What is left
 * here is bookkeeping the server cannot do for us: replacing snapshots, appending
 * deltas, tracking `seq`, and recording gate requests that must be answered.
 */
import type { ItemKind, RuntimeEvent, ThreadItem, ThreadRuntimeSnapshot, TurnState } from '../../../shared/contracts'
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
    childRunRevision: 0
  }
}

/** Deltas name their item kind by event; no content inspection is involved. */
const DELTA_ITEM_KIND: Partial<Record<RuntimeEvent['kind'], ItemKind>> = {
  assistant_text_delta: 'assistant_message',
  assistant_reasoning_delta: 'reasoning'
}

const MAX_ERROR_LINES = 50
const MAX_CHILD_COMPLETIONS = 50

function clone(state: ItemsSnapshot): ItemsSnapshot {
  return {
    itemsById: { ...state.itemsById },
    itemOrder: [...state.itemOrder],
    turnState: { ...state.turnState },
    activeTurnId: state.activeTurnId,
    lastSeq: state.lastSeq,
    errors: [...state.errors],
    approvals: { ...state.approvals },
    userInputs: { ...state.userInputs },
    childCompletions: { ...state.childCompletions },
    childRunRevision: state.childRunRevision
  }
}

/**
 * Snapshot events are authoritative, but a snapshot may be partial (status-only
 * `item_updated`). Field-level merge keeps accumulated text in that case.
 */
function upsertItem(next: ItemsSnapshot, item: ThreadItem): void {
  const existing = next.itemsById[item.item_id]
  const merged: ThreadItem = existing
    ? {
        ...existing,
        ...item,
        text: item.text ?? existing.text,
        tool: item.tool ?? existing.tool,
        detail: item.detail ?? existing.detail
      }
    : item

  if (!existing) next.itemOrder.push(item.item_id)
  next.itemsById[item.item_id] = merged
}

function appendDelta(
  next: ItemsSnapshot,
  event: Extract<RuntimeEvent, { kind: 'assistant_text_delta' | 'assistant_reasoning_delta' }>
): void {
  const existing = next.itemsById[event.item_id]
  if (existing) {
    next.itemsById[event.item_id] = {
      ...existing,
      text: (existing.text ?? '') + event.delta,
      status: existing.status === 'pending' ? 'streaming' : existing.status
    }
    return
  }

  // The runtime normally creates the item first; if a delta wins the race we
  // materialise a streaming row so no token is silently dropped.
  const kind = DELTA_ITEM_KIND[event.kind] ?? 'assistant_message'
  next.itemOrder.push(event.item_id)
  next.itemsById[event.item_id] = {
    item_id: event.item_id,
    turn_id: event.turn_id,
    kind,
    status: 'streaming',
    created_at: event.timestamp,
    text: event.delta
  }
}

function setTurn(next: ItemsSnapshot, turnId: string, state: TurnState): void {
  next.turnState[turnId] = state
  const terminal = state === 'completed' || state === 'failed' || state === 'aborted'
  if (terminal && next.activeTurnId === turnId) next.activeTurnId = null
}

function pushError(next: ItemsSnapshot, line: ErrorLine): void {
  next.errors = [...next.errors, line].slice(-MAX_ERROR_LINES)
}

/** Keep UI-only completion cards bounded even if a stream is unusually long. */
function recordChildCompletion(next: ItemsSnapshot, notice: ChildCompletionNotice): void {
  if (next.childCompletions[notice.notification_id]) return
  next.childCompletions[notice.notification_id] = notice
  const notificationIds = Object.keys(next.childCompletions)
  if (notificationIds.length <= MAX_CHILD_COMPLETIONS) return

  const oldestId = notificationIds.reduce((oldest, id) => {
    const candidate = next.childCompletions[id]
    const current = next.childCompletions[oldest]
    return candidate && current && candidate.seq < current.seq ? id : oldest
  })
  delete next.childCompletions[oldestId]
}

/** Seed history from `GET /v1/threads/{id}`. */
export function seedFromItems(
  items: ThreadItem[],
  lastSeq: number,
  childCompletions: ChildCompletionNotice[],
  runtime: ThreadRuntimeSnapshot
): ItemsSnapshot {
  const next = emptyItemsState()
  next.lastSeq = lastSeq
  for (const item of items) upsertItem(next, item)
  for (const notice of childCompletions) recordChildCompletion(next, notice)
  if (runtime.active_turn) {
    next.activeTurnId = runtime.active_turn.turn_id
    // An active registry entry can only be running or aborting. Keep an
    // unknown future wire string actionable as running rather than inventing a
    // terminal state in the browser.
    next.turnState[runtime.active_turn.turn_id] = runtime.active_turn.state === 'aborting' ? 'aborting' : 'running'
  }
  for (const approval of runtime.pending_approvals) {
    next.approvals[approval.approval_id] = { ...approval, status: 'pending' }
  }
  for (const request of runtime.pending_user_inputs) {
    next.userInputs[request.request_id] = { ...request, status: 'pending' }
  }
  return next
}

/**
 * Fold one SSE event. Unknown kinds only advance `seq`, which keeps the
 * reconnect watermark correct even for events the UI does not render.
 */
export function applyEvent(state: ItemsSnapshot, event: RuntimeEvent): ItemsSnapshot {
  const next = clone(state)
  if (event.seq > next.lastSeq) next.lastSeq = event.seq

  switch (event.kind) {
    case 'item_created':
    case 'item_updated':
    case 'item_completed':
      upsertItem(next, event.item)
      return next

    case 'assistant_text_delta':
    case 'assistant_reasoning_delta':
      appendDelta(next, event)
      return next

    case 'turn_started':
      next.activeTurnId = event.turn_id
      setTurn(next, event.turn_id, 'running')
      return next

    case 'turn_completed':
      setTurn(next, event.turn_id, 'completed')
      return next

    case 'turn_failed':
      setTurn(next, event.turn_id, 'failed')
      pushError(next, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: 'internal_error',
        message: event.message
      })
      return next

    case 'turn_aborted':
      setTurn(next, event.turn_id, 'aborted')
      return next

    case 'transient_retry':
      pushError(next, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: 'model_request_failed',
        message: `第 ${event.attempt} 次重试：${event.message}`
      })
      return next

    case 'error':
      pushError(next, {
        seq: event.seq,
        timestamp: event.timestamp,
        code: event.code,
        message: event.message
      })
      return next

    case 'approval_requested': {
      const approval: PendingApproval = {
        approval_id: event.approval_id,
        turn_id: event.turn_id,
        tool: event.tool,
        summary: event.summary,
        status: 'pending',
        requested_at: event.timestamp
      }
      next.approvals[event.approval_id] = approval
      return next
    }

    case 'approval_resolved': {
      const existing = next.approvals[event.approval_id]
      if (existing) {
        next.approvals[event.approval_id] = {
          ...existing,
          status: event.approved ? 'approved' : 'rejected'
        }
      }
      return next
    }

    case 'user_input_requested': {
      const request: PendingUserInput = {
        request_id: event.request_id,
        turn_id: event.turn_id,
        prompt: event.prompt,
        status: 'pending',
        requested_at: event.timestamp
      }
      next.userInputs[event.request_id] = request
      return next
    }

    case 'user_input_resolved': {
      const existing = next.userInputs[event.request_id]
      if (existing) {
        next.userInputs[event.request_id] = { ...existing, status: 'answered', answer: event.answer }
      }
      return next
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
      recordChildCompletion(next, notice)
      next.childRunRevision += 1
      return next
    }

    case 'child_activity':
      next.childRunRevision += 1
      return next

    default:
      return next
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
