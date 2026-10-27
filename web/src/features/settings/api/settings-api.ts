/**
 * Runtime config surface (`/v1/runtime/config`).
 *
 * Only mapping: the paths live here so no component or hook carries a route
 * of its own (design §7.10.2).
 */
import type { ApplyResult, ConfigPatch, RuntimeConfigView } from '../../../shared/contracts'
import { get, post } from '../../../shared/api/client'

/** `GET /v1/runtime/config` — the authoritative, already-normalised config. */
export function getRuntimeConfig(signal?: AbortSignal): Promise<RuntimeConfigView> {
  return get<RuntimeConfigView>('/v1/runtime/config', signal ? { signal } : {})
}

/**
 * `POST /v1/runtime/config` — partial update.
 *
 * The runtime answers `applied` or `rejected`; a rejection is a normal result,
 * not an ApiError, so the caller can surface `restart_required` verbatim.
 */
export function patchRuntimeConfig(patch: ConfigPatch): Promise<ApplyResult> {
  return post<ApplyResult>('/v1/runtime/config', patch)
}
