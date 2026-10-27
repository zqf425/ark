/**
 * Runtime liveness surface used by the top status bar.
 */
import type { HealthResponse, RuntimeInfo, UsageResponse } from '../../../shared/contracts'
import { get } from '../../../shared/api/client'

/** `GET /health` — unauthenticated, so it also verifies the proxy is alive. */
export function getHealth(signal?: AbortSignal): Promise<HealthResponse> {
  return get<HealthResponse>('/health', signal ? { signal } : {})
}

/** `GET /v1/runtime/info`. */
export function getRuntimeInfo(signal?: AbortSignal): Promise<RuntimeInfo> {
  return get<RuntimeInfo>('/v1/runtime/info', signal ? { signal } : {})
}

/** `GET /v1/usage` — provider counters only. */
export function getUsage(signal?: AbortSignal): Promise<UsageResponse> {
  return get<UsageResponse>('/v1/usage', signal ? { signal } : {})
}
