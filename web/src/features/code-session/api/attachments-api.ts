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
