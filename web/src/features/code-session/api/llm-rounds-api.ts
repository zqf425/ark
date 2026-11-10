/**
 * Bounded diagnostics surface used by the session statistics strip.
 *
 * Rounds and their `duration_ms` are folded server-side from the authoritative
 * `usage` events; the renderer only sums them, never times model calls itself.
 */
import type { LlmRoundsResponse, ThreadId } from '../../../shared/contracts'
import { get } from '../../../shared/api/client'

/** Runtime clamp (`LLM_ROUNDS_MAX_LIMIT`); one page covers a long session. */
const ROUND_PAGE_LIMIT = 500

/** `GET /v1/debug/llm-rounds?thread_id=…&limit=500`. */
export function listLlmRounds(threadId: ThreadId, signal?: AbortSignal): Promise<LlmRoundsResponse> {
  return get<LlmRoundsResponse>('/v1/debug/llm-rounds', {
    query: { thread_id: threadId, limit: ROUND_PAGE_LIMIT },
    ...(signal ? { signal } : {})
  })
}
