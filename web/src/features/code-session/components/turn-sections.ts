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
