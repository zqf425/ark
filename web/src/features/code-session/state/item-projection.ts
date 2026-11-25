/** Renderer projections shared by parent and child transcripts. */
import type { RuntimeEvent, ThreadItem } from '../../../shared/contracts'

export function projectToolCall(
  event: Extract<RuntimeEvent, { kind: 'tool_call_started' | 'tool_call_finished' }>,
  existing?: ThreadItem
): ThreadItem {
  const started = event.kind === 'tool_call_started'
  return {
    item_id: event.item_id!,
    turn_id: event.turn_id,
    kind: 'tool_call',
    status: started ? (existing?.status ?? 'streaming') : 'completed',
    created_at: existing?.created_at ?? event.timestamp,
    text: existing?.text,
    detail: started ? existing?.detail : (event.detail ?? existing?.detail),
    tool: {
      call_id: event.call_id,
      tool: event.tool,
      arguments: 'arguments' in event ? event.arguments : existing?.tool?.arguments,
      output: started ? existing?.tool?.output : (event.output || existing?.tool?.output),
      ok: started ? existing?.tool?.ok : (event.ok ?? existing?.tool?.ok),
      duration_ms: started ? existing?.tool?.duration_ms : (event.duration_ms ?? existing?.tool?.duration_ms)
    }
  }
}

export function projectUserInput(
  event: Extract<RuntimeEvent, { kind: 'user_input_requested' | 'user_input_resolved' }>,
  existing?: ThreadItem
): ThreadItem {
  const resolved = event.kind === 'user_input_resolved'
  return {
    item_id: event.item_id!,
    turn_id: event.turn_id,
    kind: 'user_input',
    status: resolved ? 'completed' : 'pending',
    created_at: existing?.created_at ?? event.timestamp,
    text: resolved ? (existing?.text ?? event.answer) : event.prompt,
    detail: resolved ? event.answer : existing?.detail
  }
}
