/**
 * Attachment surface (`/v1/attachments`).
 *
 * Upload is a Base64 JSON POST; the runtime validates size and mime. The
 * returned attachment id rides with the next turn start (`attachment_ids`).
 */
import type { AttachmentUploadRequest, AttachmentUploadResponse } from '../../../shared/contracts'
import { buildUrl, readToken } from '../../../shared/api/client'
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

/**
 * Object URLs for attachment bytes (`GET /v1/attachments/{id}/content`).
 *
 * Every runtime route sits behind the bearer token, so a plain `<img>` URL can
 * never authenticate; the bytes ride the authenticated fetch like `exportThread`
 * and turn into a `blob:` URL. Production CSP allows `blob:` images but not a
 * cross-origin runtime URL in `img-src`, which makes this the one portable path.
 */
const objectUrlCache = new Map<string, string>()

export async function fetchAttachmentObjectUrl(attachmentId: string): Promise<string> {
  const cached = objectUrlCache.get(attachmentId)
  if (cached) return cached
  const token = readToken()
  const headers: Record<string, string> = {}
  if (token) headers.authorization = `Bearer ${token}`
  const response = await fetch(buildUrl(`/v1/attachments/${attachmentId}/content`), { headers })
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  const blob = await response.blob()
  const url = URL.createObjectURL(blob)
  objectUrlCache.set(attachmentId, url)
  return url
}
