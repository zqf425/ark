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
export type TurnView = ItemView & Pick<ItemsSnapshot, 'turnState'>

export type TurnGroup = {
  turnId: TurnId
  items: ThreadItem[]
  state: ItemsSnapshot['turnState'][TurnId] | undefined
}

/**
 * Group rows into their turns in `itemOrder`.
 *
 * A group whose rows and state match `previous` elementwise is reused verbatim,
 * so the caller's `React.memo` turn block keeps its identity while other turns
 * stream. The comparison is O(rows) reference checks, which is the price of
 * skipping the render of every untouched row.
 */
export function groupByTurn(view: TurnView, previous?: readonly TurnGroup[]): TurnGroup[] {
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

  if (!previous || previous.length === 0) return groups

  const previousById = new Map<TurnId, TurnGroup>()
  for (const group of previous) previousById.set(group.turnId, group)
  return groups.map((group) => (sameGroup(previousById.get(group.turnId), group) ? (previousById.get(group.turnId) as TurnGroup) : group))
}

function sameGroup(previous: TurnGroup | undefined, next: TurnGroup): boolean {
  if (!previous) return false
  if (previous.state !== next.state) return false
  if (previous.items.length !== next.items.length) return false
  for (let i = 0; i < next.items.length; i += 1) {
    if (previous.items[i] !== next.items[i]) return false
  }
  return true
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

/** True while any item still streams — used to disable send and show a caret. */
export function hasLiveItems(view: ItemView): boolean {
  return view.itemOrder.some((itemId: ItemId) => {
    const item = view.itemsById[itemId]
    return item !== undefined && (item.status === 'pending' || item.status === 'streaming')
  })
}
