/**
 * Transcript state for one expanded child run.
 *
 * The child transcript comes from the two already-wired routes the panel never
 * used before: `GET /v1/child-runs/{id}/thread` (authoritative item snapshot)
 * and `GET /v1/child-runs/{id}/events` (incremental polling). Both reuse the
 * thread detail projection, so the rows render through the same item shapes —
 * no second event vocabulary and no Rust/TS double implementation.
 *
 * State is kept out of the zustand canvas store on purpose: only the expanded
 * row needs it, a thread switch unmounts it, and the parent stream must not
 * re-render while a child transcript folds.
 */
import type { RuntimeEvent, ThreadItem } from '../../../shared/contracts'

export type ChildTranscript = {
  /** Row ids in display order, matching the server's item projection. */
  itemOrder: string[]
  itemsById: Record<string, ThreadItem>
  /** Latest folded `seq`; the next poll resumes from here. */
  lastSeq: number
  /** Latest poll outcome for the footer line. */
  status: 'loading' | 'ready' | 'error'
}

/**
 * Fold one `RuntimeEvent` into the transcript.
 *
 * This is deliberately narrower than the parent reducer: a child transcript is
 * read-only history plus live progress, so only the row-carrying kinds matter.
 * Deltas append to the item the runtime already created (or materialize a
 * streaming row when a delta wins the race — same rule as the parent reducer).
 */
export function foldChildEvent(state: ChildTranscript, event: RuntimeEvent): ChildTranscript {
  if (event.seq <= state.lastSeq) return state
  const itemsById = { ...state.itemsById }
  const itemOrder = [...state.itemOrder]
  const upsert = (item: ThreadItem): void => {
    if (!itemsById[item.item_id]) itemOrder.push(item.item_id)
    itemsById[item.item_id] = item
  }

  switch (event.kind) {
    case 'item_created':
    case 'item_updated':
    case 'item_completed':
      upsert(event.item)
      break
    case 'assistant_text_delta':
    case 'assistant_reasoning_delta': {
      const existing = itemsById[event.item_id]
      if (existing) {
        itemsById[event.item_id] = {
          ...existing,
          text: (existing.text ?? '') + event.delta,
          status: existing.status === 'pending' ? 'streaming' : existing.status
        }
      } else {
        upsert({
          item_id: event.item_id,
          turn_id: event.turn_id,
          kind: event.kind === 'assistant_text_delta' ? 'assistant_message' : 'reasoning',
          status: 'streaming',
          created_at: event.timestamp,
          text: event.delta
        })
      }
      break
    }
    // Turn/child lifecycle events carry no row; they only move the watermark.
    default:
      break
  }
  return { itemOrder, itemsById, lastSeq: event.seq, status: state.status }
}

/** Replace the projection with an authoritative snapshot (initial load). */
export function seedChildTranscript(items: ThreadItem[], lastSeq: number): ChildTranscript {
  const itemsById: Record<string, ThreadItem> = {}
  const itemOrder: string[] = []
  for (const item of items) {
    if (itemsById[item.item_id]) continue
    itemsById[item.item_id] = item
    itemOrder.push(item.item_id)
  }
  return { itemOrder, itemsById, lastSeq, status: 'ready' }
}
