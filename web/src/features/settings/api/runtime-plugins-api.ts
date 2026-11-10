/**
 * Plugin/capability/tool diagnostics surface (`/v1/runtime/*`).
 *
 * Only mapping: the paths live here so no component or hook carries a route of
 * its own (design §7.10.2). The tools report is intentionally the web-only
 * view type: the runtime route still answers unmodelled JSON.
 */
import type {
  CapabilitiesResponse,
  PluginEnableRequest,
  PluginHostReport,
  PluginProvisionView
} from '../../../shared/contracts'
import type { RuntimeToolsReport } from '../../../shared/contracts/web-only'
import { get, post } from '../../../shared/api/client'

/** `GET /v1/runtime/capabilities`. */
export function getCapabilities(signal?: AbortSignal): Promise<CapabilitiesResponse> {
  return get<CapabilitiesResponse>('/v1/runtime/capabilities', signal ? { signal } : {})
}

/** `GET /v1/runtime/plugins`. */
export function getPluginsReport(signal?: AbortSignal): Promise<PluginHostReport> {
  return get<PluginHostReport>('/v1/runtime/plugins', signal ? { signal } : {})
}

/** `GET /v1/runtime/tools` — the built-in tool registry snapshot. */
export function getToolsReport(signal?: AbortSignal): Promise<RuntimeToolsReport> {
  return get<RuntimeToolsReport>('/v1/runtime/tools', signal ? { signal } : {})
}

/** `GET /v1/runtime/plugins/{id}/provision` — enable-flow state of one plugin. */
export function getPluginProvision(
  id: string,
  signal?: AbortSignal
): Promise<PluginProvisionView> {
  return get<PluginProvisionView>(
    `/v1/runtime/plugins/${encodeURIComponent(id)}/provision`,
    signal ? { signal } : {}
  )
}

/** `POST /v1/runtime/plugins/{id}/enable` — detect, optionally install, register MCP. */
export function enablePlugin(
  id: string,
  request: PluginEnableRequest
): Promise<PluginProvisionView> {
  return post<PluginProvisionView>(`/v1/runtime/plugins/${encodeURIComponent(id)}/enable`, request)
}
