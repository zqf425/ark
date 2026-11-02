/**
 * IM channel surface (`/v1/im/*`, requirements/im-channels.md).
 *
 * Only mapping: whitelist semantics, credential merge rules and the 20-channel
 * cap stay in `ark-im`, so this module never re-derives a rule the runtime
 * already owns. Paths live here so no component carries a route of its own.
 */
import type {
  CreateImChannelRequest,
  ImBindingListResponse,
  ImChannelListResponse,
  ImChannelView,
  ImVerifyResponse,
  ImWeixinLoginStartResponse,
  ImWeixinLoginWaitResponse,
  UpdateImChannelRequest
} from '../../../shared/contracts'
import { del, get, patch, post } from '../../../shared/api/client'

/** `GET /v1/im/channels` — every channel with its live runtime state. */
export function listImChannels(signal?: AbortSignal): Promise<ImChannelListResponse> {
  return get<ImChannelListResponse>('/v1/im/channels', signal ? { signal } : {})
}

/** `POST /v1/im/channels` — create one channel; the runtime assigns the id. */
export function createImChannel(request: CreateImChannelRequest): Promise<ImChannelView> {
  return post<ImChannelView>('/v1/im/channels', request)
}

/**
 * `PATCH /v1/im/channels/{id}` — merge an update.
 *
 * Secret fields left empty keep the stored value; an empty string clears an
 * optional text field.
 */
export function updateImChannel(
  channelId: string,
  request: UpdateImChannelRequest
): Promise<ImChannelView> {
  return patch<ImChannelView>(`/v1/im/channels/${encodeURIComponent(channelId)}`, request)
}

/** `DELETE /v1/im/channels/{id}` — 204, or 404 when the channel is gone. */
export function deleteImChannel(channelId: string): Promise<void> {
  return del<void>(`/v1/im/channels/${encodeURIComponent(channelId)}`)
}

/** `POST /v1/im/channels/{id}/verify` — the platform identity, or an error. */
export function verifyImChannel(channelId: string): Promise<ImVerifyResponse> {
  return post<ImVerifyResponse>(`/v1/im/channels/${encodeURIComponent(channelId)}/verify`, {})
}

/** `GET /v1/im/bindings` — the read-only chat→thread mapping table. */
export function listImBindings(channelId?: string, signal?: AbortSignal): Promise<ImBindingListResponse> {
  const query = channelId ? `?channel_id=${encodeURIComponent(channelId)}` : ''
  return get<ImBindingListResponse>(`/v1/im/bindings${query}`, signal ? { signal } : {})
}

/** `POST /v1/im/weixin/login/start` — fetch a QR code session. */
export function startWeixinLogin(): Promise<ImWeixinLoginStartResponse> {
  return post<ImWeixinLoginStartResponse>('/v1/im/weixin/login/start', {})
}

/** `GET /v1/im/weixin/login/wait` — poll once; confirmed creates the channel. */
export function waitWeixinLogin(sessionId: string): Promise<ImWeixinLoginWaitResponse> {
  return get<ImWeixinLoginWaitResponse>(
    `/v1/im/weixin/login/wait?session_id=${encodeURIComponent(sessionId)}`
  )
}
