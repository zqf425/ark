/**
 * Copy-on-write builder for `ItemsSnapshot`.
 *
 * A long turn folds thousands of events. The original `clone()` copied all eight
 * channels on every fold, so a single streamed token handed subscribers brand-new
 * references for `itemOrder`, `itemsById`, `turnState` and every gate map. That
 * invalidated every downstream memo and re-rendered every row of the canvas.
 *
 * A draft copies a channel only when it is written; untouched channels keep the
 * previous identity, which is what lets the canvas memoize its per-turn and
 * per-row projections on those references. `sameItemContent` closes the last
 * hole: a snapshot re-sending an item with identical content keeps the original
 * object, so the row memo still bails out.
 */
import type { ItemId, ThreadItem, TurnId, TurnState } from '../../../shared/contracts'
import type { ChildCompletionNotice, CompactionNotice, ItemsSnapshot, PendingApproval, PendingUserInput, RoundWindow, UsageAggregate } from './types'

export type ItemsDraft = {
  base: ItemsSnapshot
  itemsById: Record<ItemId, ThreadItem> | null
  itemOrder: ItemId[] | null
  turnState: Record<TurnId, TurnState> | null
  activeTurnId: TurnId | null
  lastSeq: number
  approvals: Record<string, PendingApproval> | null
  userInputs: Record<string, PendingUserInput> | null
  childCompletions: Record<string, ChildCompletionNotice> | null
  childRunRevision: number
  childRunActiveCount: number
  workbenchRevision: number
  rewindRevision: number
  rewoundFromSeq: number | null
  usage: UsageAggregate | null
  roundWindow: RoundWindow | null
  lastCompaction: CompactionNotice | null
}

export function draftOf(base: ItemsSnapshot): ItemsDraft {
  return {
    base,
    itemsById: null,
    itemOrder: null,
    turnState: null,
    activeTurnId: base.activeTurnId,
    lastSeq: base.lastSeq,
    approvals: null,
    userInputs: null,
    childCompletions: null,
    childRunRevision: base.childRunRevision,
    childRunActiveCount: base.childRunActiveCount,
    workbenchRevision: base.workbenchRevision,
    rewindRevision: base.rewindRevision,
    rewoundFromSeq: base.rewoundFromSeq,
    usage: base.usage,
    roundWindow: base.roundWindow,
    lastCompaction: base.lastCompaction
  }
}

export function draftItems(draft: ItemsDraft): Record<ItemId, ThreadItem> {
  draft.itemsById ??= { ...draft.base.itemsById }
  return draft.itemsById
}

export function draftItemOrder(draft: ItemsDraft): ItemId[] {
  draft.itemOrder ??= [...draft.base.itemOrder]
  return draft.itemOrder
}

export function draftTurnState(draft: ItemsDraft): Record<TurnId, TurnState> {
  draft.turnState ??= { ...draft.base.turnState }
  return draft.turnState
}

export function draftApprovals(draft: ItemsDraft): Record<string, PendingApproval> {
  draft.approvals ??= { ...draft.base.approvals }
  return draft.approvals
}

export function draftUserInputs(draft: ItemsDraft): Record<string, PendingUserInput> {
  draft.userInputs ??= { ...draft.base.userInputs }
  return draft.userInputs
}

export function draftChildCompletions(draft: ItemsDraft): Record<string, ChildCompletionNotice> {
  draft.childCompletions ??= { ...draft.base.childCompletions }
  return draft.childCompletions
}

export function commitDraft(draft: ItemsDraft): ItemsSnapshot {
  const base = draft.base
  return {
    itemsById: draft.itemsById ?? base.itemsById,
    itemOrder: draft.itemOrder ?? base.itemOrder,
    turnState: draft.turnState ?? base.turnState,
    activeTurnId: draft.activeTurnId,
    lastSeq: draft.lastSeq,
    approvals: draft.approvals ?? base.approvals,
    userInputs: draft.userInputs ?? base.userInputs,
    childCompletions: draft.childCompletions ?? base.childCompletions,
    childRunRevision: draft.childRunRevision,
    childRunActiveCount: draft.childRunActiveCount,
    workbenchRevision: draft.workbenchRevision,
    rewindRevision: draft.rewindRevision,
    rewoundFromSeq: draft.rewoundFromSeq,
    usage: draft.usage,
    // `roundWindow` is a value field, not a copy-on-write channel: null means
    // "window reset", so it must not fall back to the base snapshot here.
    roundWindow: draft.roundWindow,
    lastCompaction: draft.lastCompaction
  }
}

/**
 * Field-wise identity check. Every field is a primitive or a reference carried
 * over from the existing item, so an untouched update is detected without a
 * deep comparison.
 */
export function sameItemContent(left: ThreadItem, right: ThreadItem): boolean {
  return (
    left.item_id === right.item_id &&
    left.turn_id === right.turn_id &&
    left.kind === right.kind &&
    left.status === right.status &&
    left.created_at === right.created_at &&
    left.text === right.text &&
    left.tool === right.tool &&
    left.detail === right.detail &&
    left.attachments === right.attachments
  )
}
