/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Mirrors `crates/ark-contract/src/event.rs` and `crates/ark-common/src/error.rs`.
 */
import type { ArkErrorCode } from './error'
import type { ThreadId, ItemId, TurnId } from './ids'
import type { ThreadItem } from './item'
import type { Usage } from './dto'

export type EventKind =
  | 'thread_created'
  | 'thread_updated'
  | 'turn_started'
  | 'turn_completed'
  | 'turn_failed'
  | 'turn_aborted'
  | 'turn_steered'
  | 'item_created'
  | 'item_updated'
  | 'item_completed'
  | 'assistant_text_delta'
  | 'assistant_reasoning_delta'
  | 'tool_call_ready'
  | 'tool_dispatch_trace'
  | 'tool_result_upload_wait'
  | 'tool_storm_suppressed'
  | 'tool_catalog_changed'
  | 'tool_call_started'
  | 'tool_call_finished'
  | 'approval_requested'
  | 'approval_resolved'
  | 'user_input_requested'
  | 'user_input_resolved'
  | 'compaction_started'
  | 'compaction_completed'
  | 'goal_updated'
  | 'goal_cleared'
  | 'todos_updated'
  | 'todos_cleared'
  | 'pipeline_stage'
  | 'context_pressure'
  | 'bash_session_started'
  | 'bash_session_updated'
  | 'bash_session_completed'
  | 'usage'
  | 'error'
  | 'heartbeat'
  | 'llm_span_start'
  | 'llm_span_end'
  | 'fingerprint_drift'
  | 'recovery_triggered'
  | 'sse_idle_timeout'
  | 'transient_retry'
  | 'child_activity'

/**
 * Kinds the renderer deliberately ignores. They still reach the client (the
 * stream is a full audit trail), but no component reads them: the corresponding
 * display state already arrives as a `ThreadItem`.
 */
export type PassiveEventKind =
  | 'tool_call_ready'
  | 'tool_dispatch_trace'
  | 'tool_result_upload_wait'
  | 'tool_catalog_changed'
  | 'goal_updated'
  | 'goal_cleared'
  | 'todos_updated'
  | 'todos_cleared'
  | 'pipeline_stage'
  | 'context_pressure'
  | 'bash_session_started'
  | 'bash_session_updated'
  | 'bash_session_completed'
  | 'llm_span_start'
  | 'llm_span_end'
  | 'fingerprint_drift'
  | 'recovery_triggered'
  | 'child_activity'

export type RuntimeEventBase = {
  seq: number
  thread_id: ThreadId
  timestamp: string
}

export type RuntimeEvent =
  | (RuntimeEventBase & { kind: 'thread_created'; title: string })
  | (RuntimeEventBase & { kind: 'thread_updated'; title: string })
  | (RuntimeEventBase & { kind: 'turn_started'; turn_id: TurnId })
  | (RuntimeEventBase & { kind: 'turn_completed'; turn_id: TurnId })
  | (RuntimeEventBase & { kind: 'turn_failed'; turn_id: TurnId; message: string })
  | (RuntimeEventBase & { kind: 'turn_aborted'; turn_id: TurnId; reason: string })
  | (RuntimeEventBase & { kind: 'turn_steered'; turn_id: TurnId; text: string })
  | (RuntimeEventBase & { kind: 'item_created'; turn_id: TurnId; item: ThreadItem })
  | (RuntimeEventBase & { kind: 'item_updated'; turn_id: TurnId; item: ThreadItem })
  | (RuntimeEventBase & { kind: 'item_completed'; turn_id: TurnId; item: ThreadItem })
  | (RuntimeEventBase & { kind: 'assistant_text_delta'; turn_id: TurnId; item_id: ItemId; delta: string })
  | (RuntimeEventBase & {
      kind: 'assistant_reasoning_delta'
      turn_id: TurnId
      item_id: ItemId
      delta: string
    })
  | (RuntimeEventBase & { kind: 'tool_call_started'; turn_id: TurnId; call_id: string; tool: string })
  | (RuntimeEventBase & {
      kind: 'tool_call_finished'
      turn_id: TurnId
      call_id: string
      tool: string
      ok: boolean
      duration_ms: number
    })
  | (RuntimeEventBase & {
      kind: 'tool_storm_suppressed'
      turn_id: TurnId
      tool: string
      count: number
    })
  | (RuntimeEventBase & {
      kind: 'approval_requested'
      turn_id: TurnId
      approval_id: string
      tool: string
      summary: string
    })
  | (RuntimeEventBase & {
      kind: 'approval_resolved'
      turn_id: TurnId
      approval_id: string
      approved: boolean
    })
  | (RuntimeEventBase & {
      kind: 'user_input_requested'
      turn_id: TurnId
      request_id: string
      prompt: string
    })
  | (RuntimeEventBase & {
      kind: 'user_input_resolved'
      turn_id: TurnId
      request_id: string
      answer: string
    })
  | (RuntimeEventBase & { kind: 'usage'; turn_id: TurnId; usage: Usage })
  | (RuntimeEventBase & { kind: 'error'; code: ArkErrorCode; message: string })
  | (RuntimeEventBase & { kind: 'heartbeat' })
  | (RuntimeEventBase & { kind: 'sse_idle_timeout' })
  | (RuntimeEventBase & { kind: 'transient_retry'; attempt: number; message: string })
  | (RuntimeEventBase & { kind: 'compaction_started'; turn_id: TurnId })
  | (RuntimeEventBase & { kind: 'compaction_completed'; turn_id: TurnId; removed_items: number })
  | (RuntimeEventBase & { kind: PassiveEventKind })

/** Events that never change what the stream shows. */
export type PassiveRuntimeEvent = RuntimeEventBase & { kind: PassiveEventKind }
