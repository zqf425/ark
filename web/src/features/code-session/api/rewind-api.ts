/**
 * `POST /v1/threads/{id}/rewind` — the only rewind surface in the renderer.
 * Request/response stay as the generated contract; nothing here infers state.
 */
import type { RewindRequest, RewindResponse } from '../../../generated'
import type { ThreadId } from '../../../shared/contracts'
import { post } from '../../../shared/api/client'

export function rewindThread(threadId: ThreadId, request: RewindRequest): Promise<RewindResponse> {
  return post<RewindResponse>(`/v1/threads/${threadId}/rewind`, request)
}
