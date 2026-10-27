/**
 * Provider surface (`/v1/runtime/providers`).
 *
 * Every mutate call answers with the full list, so the UI never has to guess
 * the new `default_provider_id` / `active_provider_id`.
 */
import type {
  ProviderListResponse,
  ProviderTestRequest,
  ProviderTestResponse,
  ProviderUpsertRequest
} from '../../../shared/contracts'
import { del, get, post } from '../../../shared/api/client'

/** `GET /v1/runtime/providers`. */
export function listProviders(signal?: AbortSignal): Promise<ProviderListResponse> {
  return get<ProviderListResponse>('/v1/runtime/providers', signal ? { signal } : {})
}

/** `POST /v1/runtime/providers` — create or replace one entry by id. */
export function upsertProvider(request: ProviderUpsertRequest): Promise<ProviderListResponse> {
  return post<ProviderListResponse>('/v1/runtime/providers', request)
}

/** `DELETE /v1/runtime/providers/{id}`. */
export function deleteProvider(providerId: string): Promise<ProviderListResponse> {
  return del<ProviderListResponse>(`/v1/runtime/providers/${encodeURIComponent(providerId)}`)
}

/** `POST /v1/runtime/providers/{id}/default`. */
export function setDefaultProvider(providerId: string): Promise<ProviderListResponse> {
  return post<ProviderListResponse>(`/v1/runtime/providers/${encodeURIComponent(providerId)}/default`, {})
}

/** `POST /v1/runtime/providers/test` — probe a stored id or an unsaved entry. */
export function testProvider(request: ProviderTestRequest): Promise<ProviderTestResponse> {
  return post<ProviderTestResponse>('/v1/runtime/providers/test', request)
}
