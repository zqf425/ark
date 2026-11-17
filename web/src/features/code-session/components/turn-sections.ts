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
 * How many of the newest `tool_call` rows stay expanded by default in a
 * settled turn's process card; older ones fold behind one summary row
 * (`splitProcessTrace`). Pure display policy: the rows all stay in the store.
 */
export const RECENT_TOOL_ROW_LIMIT = 3

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
 * View of the work trace after the default tool-call fold: the trace is cut
 * at the fold point into the untouched prefix (older narration plus the folded
 * tool rows) and the tail (the newest tool rows plus later non-tool rows).
 * `TurnBlock` derives the visible rows from these slices.
 */
export type ProcessTraceView = {
  /** Everything before the fold point, in real event order. */
  prefix: ProcessItem[]
  /** How many `tool_call` rows inside `prefix` the default fold hides. */
  foldedToolCalls: number
  /** The newest tool rows plus every non-tool row after the fold point. */
  tail: ProcessItem[]
}

/**
 * Default view of the work trace: only the newest few `tool_call` rows stay
 * expanded, older ones fold behind one summary row. The cut preserves the real
 * event order — non-tool rows (narration, reasoning, results, errors) stay on
 * the side of the fold point they happened on, so the summary row reads as
 * "earlier tool calls", not as a rewritten history.
 *
 * Runs with at most `recentLimit` tool rows pass through unchanged. The fold
 * only ever moves *earlier* calls: while the turn runs the newest tool row
 * must stay visible as it streams, and `TurnBlock` keeps the fold open while
 * `live`.
 */
export function splitProcessTrace(
  process: readonly ProcessItem[],
  recentLimit: number
): ProcessTraceView {
  const toolIndices: number[] = []
  for (const [index, item] of process.entries()) {
    if (item.kind === 'tool_call') toolIndices.push(index)
  }
  if (toolIndices.length <= recentLimit) {
    return { prefix: [...process], foldedToolCalls: 0, tail: [] }
  }

  const foldedCount = toolIndices.length - recentLimit
  const foldAt = toolIndices[foldedCount] as number
  return {
    prefix: process.slice(0, foldAt),
    foldedToolCalls: foldedCount,
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
