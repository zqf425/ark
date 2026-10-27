/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Mirrors `crates/ark-common/src/usage.rs` and `crates/ark-contract/src/dto.rs`.
 */
import type { ThreadId, TurnId } from './ids'
import type { ThreadItem } from './item'

/** Provider-native counters only; derived rates are never guessed client-side. */
export type Usage = {
  prompt_tokens: number
  completion_tokens: number
  total_tokens: number
  cache_hit_tokens?: number
  cache_miss_tokens?: number
  cache_hit_rate?: number
  cost_usd?: number
}

/** `GET /health` — unauthenticated liveness probe. */
export type HealthResponse = {
  status: string
  version: string
  uptime_ms: number
}

/** `GET /v1/runtime/info`. */
export type RuntimeInfo = {
  runtime: string
  version: string
  data_dir: string
  thread_count: number
  started_at: string
}

/** One row of `GET /v1/threads`. */
export type ThreadSummary = {
  thread_id: ThreadId
  title: string
  created_at: string
  updated_at: string
  last_seq?: number
  archived?: boolean
}

/** `POST /v1/threads`. */
export type CreateThreadRequest = {
  title?: string
}

/** `PATCH /v1/threads/{id}`. */
export type UpdateThreadRequest = {
  title?: string
  archived?: boolean
}

/** `GET /v1/threads/{id}` — metadata plus the render-ready item stream. */
export type ThreadDetail = {
  summary: ThreadSummary
  items: ThreadItem[]
  child_ids?: string[]
}

/** `POST /v1/threads/{id}/turns`. */
export type TurnRequest = {
  text: string
}

/** State machine values the loop reports for one turn. */
export type TurnState = 'running' | 'completed' | 'failed' | 'aborted' | 'pending'

/** `GET /v1/threads/{id}/turns/{turnId}`. */
export type TurnSummary = {
  turn_id: TurnId
  thread_id: ThreadId
  state: TurnState
  started_at: string
  ended_at?: string
  error?: string
}

/** `GET /v1/usage`. */
export type UsageResponse = Usage & {
  turns: number
}
