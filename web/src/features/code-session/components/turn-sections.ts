/**
 * Turn rows → process card + reply, preserving the real event order.
 *
 * The process card used to be fed only by reasoning/tool items while every
 * assistant message rendered after it, so a tool that actually ran *after* a
 * narrating reply displayed *before* it. The rule is chronological:
 * everything except the single final assistant answer belongs inside the
 * process trace, in event order.
 */

/** Kinds that make up the work trace when they are not the final reply. */
export type ProcessItem = import('../../../shared/contracts').ThreadItem

/** True while the turn is still producing rows (no final reply yet). */
export type TurnRunState = 'running' | 'settled'

/**
 * How many recent reasoning/tool rows stay visible inside the process card.
 * Everything earlier folds behind one summary row (`splitProcessTrace`). Pure
 * display policy: every row still lives in the store, and the summary row is
 * always the way back to the full trace.
 */
export const RECENT_ACTIVITY_ROW_LIMIT = 3

/**
 * Rows that represent what the agent *did* rather than what it said: a
 * reasoning block or a tool call/result. These are the rows a running turn
 * keeps only the newest few of, so a long turn reads as its latest activity
 * instead of a wall of cards.
 */
export function isActivityRow(item: ProcessItem): boolean {
  return item.kind === 'reasoning' || item.kind === 'tool_call' || item.kind === 'tool_result'
}

/**
 * Rows the default fold hides behind its summary row: everything in the
 * earlier slice except the rows that must stay visible by contract. Failures
 * and notices render at the interruption point (docs/modules/web.md), so a
 * long turn's earlier steps fold away while its errors and system notices do
 * not. Activity, mid-turn narration and any other descriptive row fold.
 */
export function isFoldedProcessRow(item: ProcessItem): boolean {
  return item.kind !== 'error' && item.kind !== 'notice'
}

/**
 * The single assistant message rendered as the reply bubble.
 *
 * The last assistant item
 * that carries visible (non-blank) text. Narration between tool rounds and
 * blank/aborted assistant rows stay in the process card.
 */
export function finalReplyIndex(items: readonly ProcessItem[], run: TurnRunState): number {
  if (run === 'running') return -1
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index]
    if (!item) continue
    if (item.kind === 'assistant_message' && (item.text ?? '').trim()) return index
  }
  return -1
}

export type TurnSections = {
  /** Rows in original order: user message first, then the process trace. */
  user: ProcessItem[]
  /** Work trace in event order: reasoning, tools, intermediate narration. */
  process: ProcessItem[]
  /** The turn's final reply; empty while the turn runs. */
  reply: ProcessItem[]
}

/**
 * View of the work trace after the default fold: the trace is cut at the fold
 * point into the earlier slice (folded activity plus the narration around it)
 * and the tail (the newest activity rows plus everything after them).
 * `TurnBlock` derives the visible rows from these slices.
 */
export type ProcessTraceView = {
  /** Everything before the fold point, in real event order. */
  prefix: ProcessItem[]
  /** How many activity rows inside `prefix` the default fold hides. */
  foldedActivityRows: number
  /** The newest activity rows plus every row after the fold point. */
  tail: ProcessItem[]
}

/**
 * Default view of the work trace: only the newest `recentLimit` activity rows
 * (reasoning, tool calls, tool results) stay visible, earlier ones fold behind
 * one summary row. The cut preserves the real event order — narration, errors
 * and notices stay on the side of the fold point they happened on, so the
 * summary row reads as "earlier steps", not as a rewritten history.
 *
 * The fold applies while the turn runs too: a long turn must scan by its
 * current activity, and the row that is streaming right now is always the
 * newest activity row, so it stays in the tail by construction.
 */
export function splitProcessTrace(
  process: readonly ProcessItem[],
  recentLimit: number
): ProcessTraceView {
  const activityIndices: number[] = []
  for (const [index, item] of process.entries()) {
    if (isActivityRow(item)) activityIndices.push(index)
  }
  if (activityIndices.length <= recentLimit) {
    return { prefix: [...process], foldedActivityRows: 0, tail: [] }
  }

  const foldedCount = activityIndices.length - recentLimit
  const foldAt = activityIndices[foldedCount] as number
  return {
    prefix: process.slice(0, foldAt),
    foldedActivityRows: foldedCount,
    tail: process.slice(foldAt)
  }
}

/**
 * Split one turn's rows. While the turn runs nothing is final yet, so every
 * row is process; once settled, the last textful assistant message becomes the
 * reply and everything else keeps its real order inside the card.
 */
export function splitTurnSections(items: readonly ProcessItem[], run: TurnRunState): TurnSections {
  const user: ProcessItem[] = []
  const process: ProcessItem[] = []
  const reply: ProcessItem[] = []
  const finalIndex = finalReplyIndex(items, run)
  for (const [index, item] of items.entries()) {
    if (item.kind === 'user_message') user.push(item)
    else if (index === finalIndex) reply.push(item)
    else process.push(item)
  }
  return { user, process, reply }
}
