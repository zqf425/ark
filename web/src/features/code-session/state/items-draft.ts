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
import type { CompactionNotice, ItemsSnapshot, PendingApproval, PendingUserInput, PlanView, RoundTiming, RoundWindow, UsageAggregate, UsageRequest, UsageTailNode } from './types'

export type ItemsDraft = {
  base: ItemsSnapshot
  itemsById: Record<ItemId, ThreadItem> | null
  itemOrder: ItemId[] | null
  turnState: Record<TurnId, TurnState> | null
  activeTurnId: TurnId | null
  lastSeq: number
  approvals: Record<string, PendingApproval> | null
  userInputs: Record<string, PendingUserInput> | null
  childRunRevision: number
  childRunActiveCount: number
  childRunStatuses: Record<string, string> | null
  workbenchRevision: number
  rewindRevision: number
  rewoundFromSeq: number | null
  usage: UsageAggregate | null
  usageTurns: number | null
  usageRequest: UsageRequest | null
  usageTail: UsageTailNode | null
  usageSeedSeq: number
  roundWindow: RoundWindow | null
  lastCompaction: CompactionNotice | null
  rounds: RoundTiming[]
  roundIndex: Record<TurnId, number> | null
  pendingGateCounts: Record<TurnId, number> | null
  turnLastEventAt: Record<TurnId, number> | null
  changedItemIds: ItemId[]
  itemOrderChanged: boolean
  liveItemCount: number
  plan: PlanView | null
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
    childRunRevision: base.childRunRevision,
    childRunActiveCount: base.childRunActiveCount,
    childRunStatuses: null,
    workbenchRevision: base.workbenchRevision,
    rewindRevision: base.rewindRevision,
    rewoundFromSeq: base.rewoundFromSeq,
    usage: base.usage,
    usageTurns: base.usageTurns,
    usageRequest: base.usageRequest,
    usageTail: base.usageTail,
    usageSeedSeq: base.usageSeedSeq,
    roundWindow: base.roundWindow,
    lastCompaction: base.lastCompaction,
    rounds: base.rounds,
    roundIndex: null,
    pendingGateCounts: null,
    turnLastEventAt: null,
    changedItemIds: [],
    itemOrderChanged: false,
    liveItemCount: base.liveItemCount,
    plan: null
  }
}

const recordDepths = new WeakMap<object, number>()
const MAX_RECORD_LAYERS = 64

function materializeRecord<T>(record: Record<string, T>): Record<string, T> {
  const layers: Record<string, T>[] = []
  let layer: object | null = record
  while (layer && layer !== Object.prototype) {
    layers.push(layer as Record<string, T>)
    layer = Object.getPrototypeOf(layer)
  }
  return Object.assign({}, ...layers.reverse())
}

/** O(1) writes over a prior map, with periodic bounded-depth folding. */
function draftRecord<T>(base: Record<string, T>): Record<string, T> {
  const depth = recordDepths.get(base) ?? 0
  const next =
    depth >= MAX_RECORD_LAYERS
      ? materializeRecord(base)
      : (Object.create(base) as Record<string, T>)
  recordDepths.set(next, depth >= MAX_RECORD_LAYERS ? 1 : depth + 1)
  return next
}

/** O(1) delta writes over the prior map, with periodic bounded-depth folding. */
export function draftItems(draft: ItemsDraft): Record<ItemId, ThreadItem> {
  draft.itemsById ??= draftRecord(draft.base.itemsById)
  return draft.itemsById
}

export function draftItemOrder(draft: ItemsDraft): ItemId[] {
  draft.itemOrder ??= [...draft.base.itemOrder]
  draft.itemOrderChanged = true
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

export function draftChildRunStatuses(draft: ItemsDraft): Record<string, string> {
  draft.childRunStatuses ??= { ...draft.base.childRunStatuses }
  return draft.childRunStatuses
}

export function draftPlan(draft: ItemsDraft): PlanView {
  draft.plan ??= { ...draft.base.plan, todos: [...draft.base.plan.todos] }
  return draft.plan
}

/** Copy-on-write channel for the per-turn newest-event timestamps. */
export function draftTurnLastEventAt(draft: ItemsDraft): Record<TurnId, number> {
  draft.turnLastEventAt ??= draftRecord(draft.base.turnLastEventAt)
  return draft.turnLastEventAt
}

export function draftRoundIndex(draft: ItemsDraft): Record<TurnId, number> {
  draft.roundIndex ??= { ...draft.base.roundIndex }
  return draft.roundIndex
}

export function draftPendingGateCounts(draft: ItemsDraft): Record<TurnId, number> {
  draft.pendingGateCounts ??= { ...draft.base.pendingGateCounts }
  return draft.pendingGateCounts
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
    childRunRevision: draft.childRunRevision,
    childRunActiveCount: draft.childRunActiveCount,
    childRunStatuses: draft.childRunStatuses ?? base.childRunStatuses,
    workbenchRevision: draft.workbenchRevision,
    rewindRevision: draft.rewindRevision,
    rewoundFromSeq: draft.rewoundFromSeq,
    usage: draft.usage,
    usageTurns: draft.usageTurns,
    usageRequest: draft.usageRequest,
    usageTail: draft.usageTail,
    usageSeedSeq: draft.usageSeedSeq,
    // `roundWindow` is a value field, not a copy-on-write channel: null means
    // "window reset", so it must not fall back to the base snapshot here.
    roundWindow: draft.roundWindow,
    lastCompaction: draft.lastCompaction,
    rounds: draft.rounds,
    roundIndex: draft.roundIndex ?? base.roundIndex,
    pendingGateCounts: draft.pendingGateCounts ?? base.pendingGateCounts,
    turnLastEventAt: draft.turnLastEventAt ?? base.turnLastEventAt,
    changedItemIds: draft.changedItemIds,
    itemOrderChanged: draft.itemOrderChanged,
    liveItemCount: draft.liveItemCount,
    plan: draft.plan ?? base.plan
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
    left.attachments === right.attachments &&
    left.model === right.model
  )
}
