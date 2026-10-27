/**
 * MCP server surface (`/v1/mcp/servers`).
 *
 * `test` is separate from connect: it reports the tool list and latency without
 * mutating the stored entry.
 */
import type { McpServerListResponse, McpServerUpsertRequest, McpTestResult } from '../../../shared/contracts'
import { del, get, post } from '../../../shared/api/client'

/** `GET /v1/mcp/servers`. */
export function listMcpServers(signal?: AbortSignal): Promise<McpServerListResponse> {
  return get<McpServerListResponse>('/v1/mcp/servers', signal ? { signal } : {})
}

/** `POST /v1/mcp/servers` — create or replace one server by id. */
export function upsertMcpServer(request: McpServerUpsertRequest): Promise<McpServerListResponse> {
  return post<McpServerListResponse>('/v1/mcp/servers', request)
}

/** `DELETE /v1/mcp/servers/{id}`. */
export function deleteMcpServer(serverId: string): Promise<McpServerListResponse> {
  return del<McpServerListResponse>(`/v1/mcp/servers/${encodeURIComponent(serverId)}`)
}

/** `POST /v1/mcp/servers/{id}/test`. */
export function testMcpServer(serverId: string): Promise<McpTestResult> {
  return post<McpTestResult>(`/v1/mcp/servers/${encodeURIComponent(serverId)}/test`, {})
}
