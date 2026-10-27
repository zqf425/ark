/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Mirrors `crates/ark-contract/src/item.rs`.
 *
 * Mapping from raw events to these render-ready rows happens in Rust
 * (design §7.10.3). The renderer must only group and order them.
 */
import type { ItemId, TurnId } from './ids'

export type ItemKind =
  | 'user_message'
  | 'assistant_message'
  | 'reasoning'
  | 'tool_call'
  | 'tool_result'
  | 'approval'
  | 'user_input'
  | 'error'
  | 'notice'

export type ItemStatus = 'pending' | 'streaming' | 'completed' | 'failed' | 'aborted'

/** Facets carried by `tool_call` / `tool_result` items. */
export type ToolPayload = {
  call_id: string
  tool: string
  arguments?: unknown
  output?: string
  ok?: boolean
  duration_ms?: number
}

/** One render-ready row of the conversation stream. */
export type ThreadItem = {
  item_id: ItemId
  turn_id: TurnId
  kind: ItemKind
  status: ItemStatus
  created_at: string
  text?: string
  tool?: ToolPayload
  detail?: string
}

/** True while the item still expects more content from the runtime. */
export function isItemLive(item: ThreadItem): boolean {
  return item.status === 'pending' || item.status === 'streaming'
}
