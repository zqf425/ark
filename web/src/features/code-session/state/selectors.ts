/**
 * Derived views over the item snapshot.
 *
 * Deliberately shallow: grouping by turn and preserving server order. No status
 * inference, no re-sorting by kind, no text post-processing (design §7.10.3).
 *
 * Every view below reads only the channels it needs (`Pick` of `ItemsSnapshot`),
 * so a caller can subscribe to those channels individually and skip the work
 * entirely while they keep their identity. `groupByTurn` goes one step further
 * and reuses the previous group object when a turn's rows did not change: that
 * is what lets a memoized turn block bail out while one token streams.
 */
import type { ItemId, ThreadItem, TurnId } from '../../../shared/contracts'
import type { ItemsSnapshot, PendingApproval, PendingUserInput } from './types'

/** Channels needed to walk the ordered stream. */
export type ItemView = Pick<ItemsSnapshot, 'itemOrder' | 'itemsById'>

/** Channels needed to group rows into their turns. */
export type TurnView = ItemView &
  Pick<ItemsSnapshot, 'turnState'> &
  Partial<Pick<ItemsSnapshot, 'changedItemIds' | 'itemOrderChanged'>>

export type TurnGroup = {
  turnId: TurnId
  items: ThreadItem[]
  state: ItemsSnapshot['turnState'][TurnId] | undefined
}

/** Update only turns named by the reducer's changed-item set, in O(groups + changed rows). */
function updateTurnGroups(view: TurnView, previous: readonly TurnGroup[]): TurnGroup[] {
  const changedTurns = new Set<TurnId>()
  const replacements = new Map<ItemId, ThreadItem>()
  for (const itemId of view.changedItemIds ?? []) {
    const item = view.itemsById[itemId]
    if (item) {
      changedTurns.add(item.turn_id)
      replacements.set(itemId, item)
    }
  }
  return previous.map((group) => {
    const state = view.turnState[group.turnId]
    if (!changedTurns.has(group.turnId) && state === group.state) return group
    const items = changedTurns.has(group.turnId)
      ? group.items.map((item) => replacements.get(item.item_id) ?? item)
      : group.items
    return { turnId: group.turnId, items, state }
  })
}

/** Rebuild server order while retaining every structurally unchanged group. */
function rebuildTurnGroups(
  view: TurnView,
  previous: readonly TurnGroup[]
): TurnGroup[] {
  const groups: TurnGroup[] = []
  const index = new Map<TurnId, TurnGroup>()
  for (const itemId of view.itemOrder) {
    const item = view.itemsById[itemId]
    if (!item) continue
    let group = index.get(item.turn_id)
    if (!group) {
      group = { turnId: item.turn_id, items: [], state: view.turnState[item.turn_id] }
      index.set(item.turn_id, group)
      groups.push(group)
    }
    group.items.push(item)
  }
  const prior = new Map(previous.map((group) => [group.turnId, group]))
  return groups.map((group) => {
    const old = prior.get(group.turnId)
    const unchanged =
      old !== undefined &&
      old.state === group.state &&
      old.items.length === group.items.length &&
      old.items.every((item, at) => item === group.items[at])
    return unchanged ? old : group
  })
}

/**
 * Group rows into turns. Insertions rebuild the order once; later deltas use
 * reducer metadata to retain every untouched group without scanning the stream.
 * `previousView` lets callers validate that the guarded fast path still has the
 * same ordered-row channel.
 */
export function groupByTurn(
  view: TurnView,
  previous?: readonly TurnGroup[],
  previousView?: TurnView
): TurnGroup[] {
  if (previous?.length && view.changedItemIds && view.itemOrderChanged !== true) {
    const sameOrder = previousView?.itemOrder === view.itemOrder
    // The normal streaming path carries the prior view and the same ordered
    // channel; its length already proves the group count without walking every
    // historical turn. Callers without that cache retain the defensive check.
    const previousCount = sameOrder
      ? view.itemOrder.length
      : previous.reduce((count, group) => count + group.items.length, 0)
    if (sameOrder && previousCount === view.itemOrder.length) {
      return updateTurnGroups(view, previous)
    }
  }
  return rebuildTurnGroups(view, previous ?? [])
}

/** Flattened stream in display order (used by the scroll manager). */
export function orderedItems(view: ItemView): ThreadItem[] {
  const rows: ThreadItem[] = []
  for (const itemId of view.itemOrder) {
    const item = view.itemsById[itemId]
    if (item) rows.push(item)
  }
  return rows
}

/**
 * Split a fork's inherited prefix from the rows the canvas should render.
 *
 * `fork_thread` copies the source event `(0, fork_at_seq]`, so a fork opens
 * with a verbatim prefix of its parent's transcript. `inherited_item_count`
 * records exactly how many leading render rows that prefix produced. Folding
 * those rows keeps a fork from looking like a duplicate of its source; the rows
 * stay available behind a disclosure control in the canvas.
 */
export function splitInheritedPrefix(
  view: ItemView,
  count: number
): { visible: ItemView; inheritedCount: number } {
  const inheritedCount = Math.max(0, Math.min(count, view.itemOrder.length))
  if (inheritedCount === 0) return { visible: view, inheritedCount: 0 }
  return {
    visible: { itemOrder: view.itemOrder.slice(inheritedCount), itemsById: view.itemsById },
    inheritedCount
  }
}

/** Both pending and resolved approvals stay visible for the current session. */
export function approvalCards(view: Pick<ItemsSnapshot, 'approvals'>): PendingApproval[] {
  return Object.values(view.approvals).sort((a, b) => Date.parse(a.requested_at) - Date.parse(b.requested_at))
}

/** Both pending and answered prompts stay visible for the current session. */
export function userInputCards(view: Pick<ItemsSnapshot, 'userInputs'>): PendingUserInput[] {
  return Object.values(view.userInputs).sort((a, b) => Date.parse(a.requested_at) - Date.parse(b.requested_at))
}

/**
 * Answered user-input rows anchored into their own turns, in stream order.
 *
 * A settled gate is history, not a live control: pinning every answered card
 * below the newest turn kept a stale "需要你的输入 / 已回答" panel glued to
 * the bottom forever. The durable transcript already folds the gate into a
 * `user_input` row inside its turn (ark-store projects `user_input_*` to
 * `keyed_item_id("user_input:{request_id}")`), so this projection walks
 * `itemOrder`: the summary renders where the conversation asked the question,
 * and survives a seed-from-detail just like any other row. Pending gates stay
 * rendered from `userInputCards` at the stream bottom — they are the live
 * "answer me" surface. The turn anchor itself is resolved by TurnBlock off its
 * own `group.items` (memoized turn groups make the per-turn filter O(rows of
 * that turn) on identity change only); this selector is the canvas-wide
 * projection for callers that need the anchored rows as a list.
 */
export function answeredGateCards(
  view: ItemView
): Array<{ turnId: TurnId; itemId: ItemId; item: ThreadItem }> {
  const cards: Array<{ turnId: TurnId; itemId: ItemId; item: ThreadItem }> = []
  for (const itemId of view.itemOrder) {
    const item = view.itemsById[itemId]
    if (item && item.kind === 'user_input' && item.status === 'completed') {
      cards.push({ turnId: item.turn_id, itemId, item })
    }
  }
  return cards
}

/** True while any item still streams — uses the reducer's constant-time count. */
export function hasLiveItems(view: ItemView & Pick<ItemsSnapshot, 'liveItemCount'>): boolean {
  return view.liveItemCount > 0
}
