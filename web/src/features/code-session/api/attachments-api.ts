/**
 * Attachment surface (`/v1/attachments`).
 *
 * Upload is a Base64 JSON POST; the runtime validates size and mime. The
 * returned attachment id rides with the next turn start (`attachment_ids`).
 */
import type { AttachmentUploadRequest, AttachmentUploadResponse } from '../../../shared/contracts'
import { post } from '../../../shared/api/client'

/** `POST /v1/attachments`. */
export function uploadAttachment(request: AttachmentUploadRequest): Promise<AttachmentUploadResponse> {
  return post<AttachmentUploadResponse>('/v1/attachments', request)
}

/**
 * `POST /v1/threads/{id}/attachments` — authorize the attachment for the
 * thread. Idempotent server-side; a turn start rejects unassociated ids, so
 * this must land before the turn request.
 */
export function associateAttachment(threadId: string, attachmentId: string): Promise<void> {
  return post<void>(`/v1/threads/${threadId}/attachments`, { attachment_id: attachmentId })
}
