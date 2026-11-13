/**
 * The newest in-flight item of the active turn, for the live activity row.
 *
 * The work-process card keeps the durable history (design §7.10.3), but the
 * card sits above the assistant rows while the canvas auto-scrolls to the
 * bottom — during a pure reasoning stretch nothing on screen moves, which
 * reads as "hung". This projection answers "alive or stuck" from data the
 * store already folds: no new wire contract, no Rust duplicate.
 */
import type { ThreadItem, TurnId } from '../../../shared/contracts'

/** Channels the projection needs (same pattern as the other selectors). */
export type ActivityView = Pick<
  import('../state/types').ItemsSnapshot,
  'itemOrder' | 'itemsById' | 'turnState' | 'activeTurnId'
>

/** What the live activity row renders for the current instant. */
export type LiveActivity = {
  turnId: TurnId
  /** Reasoning trails and tool labels read the item's accumulated text/tool. */
  item: ThreadItem
}

/** Reasoning trails show roughly one line; longer tails are noise. */
export const REASONING_TRAIL_CHARS = 140

/** The latest item of the active turn that is still pending or streaming. */
export function liveActivity(view: ActivityView): LiveActivity | null {
  if (!view.activeTurnId) return null
  if (view.turnState[view.activeTurnId] !== 'running') return null
  let latest: ThreadItem | null = null
  for (const itemId of view.itemOrder) {
    const item = view.itemsById[itemId]
    if (!item || item.turn_id !== view.activeTurnId) continue
    if (item.status !== 'streaming' && item.status !== 'pending') continue
    latest = item
  }
  return latest ? { turnId: view.activeTurnId, item: latest } : null
}

/** Tail of a reasoning stream: keep the end, drop the settled prefix. */
export function reasoningTrail(text: string, maxChars = REASONING_TRAIL_CHARS): string {
  const trimmed = text.trim()
  if (trimmed.length <= maxChars) return trimmed
  return trimmed.slice(-maxChars)
}

/** `m:ss` elapsed readout; an unparseable timestamp shows no clock at all. */
export function formatElapsed(startedAt: string, nowMs: number): string | null {
  const start = Date.parse(startedAt)
  if (!Number.isFinite(start)) return null
  const seconds = Math.max(0, Math.floor((nowMs - start) / 1000))
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
}
