/**
 * Derived views over the item snapshot.
 *
 * Deliberately shallow: grouping by turn and preserving server order. No status
 * inference, no re-sorting by kind, no text post-processing (design §7.10.3).
 */
import type { ItemId, ThreadItem, TurnId } from '../../../shared/contracts'
import type { ChildCompletionNotice, ErrorLine, ItemsSnapshot, PendingApproval, PendingUserInput } from './types'

export type TurnGroup = {
  turnId: TurnId
  items: ThreadItem[]
  state: ItemsSnapshot['turnState'][TurnId] | undefined
}

function timestampOf(item: ThreadItem): number {
  const parsed = Date.parse(item.created_at)
  return Number.isFinite(parsed) ? parsed : 0
}

/**
 * Order is the server's order (`itemOrder`), which is the SSE arrival order,
 * itself the writer's `seq` order. Turns are grouped but never reordered.
 * Items whose turn id is unknown form their own trailing group.
 */
export function groupByTurn(snapshot: ItemsSnapshot): TurnGroup[] {
  const groups: TurnGroup[] = []
  const index = new Map<TurnId, TurnGroup>()

  for (const itemId of snapshot.itemOrder) {
    const item = snapshot.itemsById[itemId]
    if (!item) continue
    let group = index.get(item.turn_id)
    if (!group) {
      group = { turnId: item.turn_id, items: [], state: snapshot.turnState[item.turn_id] }
      index.set(item.turn_id, group)
      groups.push(group)
    }
    group.items.push(item)
  }

  return groups
}

/** Flattened stream in display order (used by the scroll manager). */
export function orderedItems(snapshot: ItemsSnapshot): ThreadItem[] {
  const rows: ThreadItem[] = []
  for (const itemId of snapshot.itemOrder) {
    const item = snapshot.itemsById[itemId]
    if (item) rows.push(item)
  }
  return rows
}

/** Both pending and resolved approvals stay visible for the current session. */
export function approvalCards(snapshot: ItemsSnapshot): PendingApproval[] {
  return Object.values(snapshot.approvals)
    .sort((a, b) => Date.parse(a.requested_at) - Date.parse(b.requested_at))
}

/** Both pending and answered prompts stay visible for the current session. */
export function userInputCards(snapshot: ItemsSnapshot): PendingUserInput[] {
  return Object.values(snapshot.userInputs)
    .sort((a, b) => Date.parse(a.requested_at) - Date.parse(b.requested_at))
}

/** Latest error lines, oldest first; the stream renders them as a footer block. */
export function recentErrors(snapshot: ItemsSnapshot, limit = 5): ErrorLine[] {
  return snapshot.errors.slice(-limit)
}

/** Completed background children in their durable event order. */
export function childCompletionNotices(snapshot: ItemsSnapshot): ChildCompletionNotice[] {
  return Object.values(snapshot.childCompletions).sort((left, right) => left.seq - right.seq)
}

/** True while any item still streams — used to disable send and show a caret. */
export function hasLiveItems(snapshot: ItemsSnapshot): boolean {
  return snapshot.itemOrder.some((itemId: ItemId) => {
    const item = snapshot.itemsById[itemId]
    return item !== undefined && (item.status === 'pending' || item.status === 'streaming')
  })
}

/** Turn id of the newest user message, used as a stable scroll anchor. */
export function lastUserTurnId(snapshot: ItemsSnapshot): TurnId | null {
  for (let i = snapshot.itemOrder.length - 1; i >= 0; i -= 1) {
    const itemId = snapshot.itemOrder[i]
    if (!itemId) continue
    const item = snapshot.itemsById[itemId]
    if (item?.kind === 'user_message') return item.turn_id
  }
  return null
}

/** Secondary sort only for equal timestamps; keeps deterministic rendering. */
export function stableTimeOrder(items: ThreadItem[]): ThreadItem[] {
  return [...items].sort((a, b) => timestampOf(a) - timestampOf(b))
}
