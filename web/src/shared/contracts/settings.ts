/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Mirrors `crates/ark-contract/src/settings.rs` (runtime config, providers,
 * workspace probe, MCP servers and skills). snake_case is the wire format.
 */

/** Concurrency window the runtime enforces for subagents (`1..=8`). */
export const SUBAGENT_MIN_CONCURRENCY = 1
export const SUBAGENT_MAX_CONCURRENCY = 8

/** `GET /v1/runtime/config`. */
export type RuntimeConfig = {
  version: number
  port: number
  data_dir: string
  log_level: string
  default_workspace: string | null
  subagents: SubagentConfig
  tuning: Record<string, unknown>
}

/** `subagents` block of the runtime config. */
export type SubagentConfig = {
  enabled: boolean
  max_concurrency: number
}

/** `POST /v1/runtime/config` — every field optional, only the present ones change. */
export type ConfigPatch = {
  default_workspace?: string
  subagents?: SubagentConfig
  log_level?: string
  tuning?: Record<string, unknown>
  port?: number
  model_providers?: Record<string, unknown> | null
}

/** `POST /v1/runtime/config` response: applied, or rejected with restart hints. */
export type ConfigApplyResult =
  | { result: 'applied'; version: number; changed: string[] }
  | { result: 'rejected'; restart_required: string[]; reason: string }

/** Log levels the runtime accepts for `log_level`. */
export const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error', 'off'] as const
export type LogLevel = (typeof LOG_LEVELS)[number]

/** Provider wire protocol families understood by the model adapter. */
export const ENDPOINT_FORMATS = ['openai_chat', 'openai_responses', 'anthropic_messages'] as const
export type EndpointFormat = (typeof ENDPOINT_FORMATS)[number]

/** One provider as the runtime stores it; `api_key` is write-only. */
export type ProviderEntry = {
  id: string
  label?: string
  base_url: string
  api_key?: string
  endpoint_format: EndpointFormat
  model: string
  enabled: boolean
  headers?: Record<string, string>
  max_tokens?: number
  temperature?: number
}

/**
 * One provider as the runtime reports it. The key never comes back: only
 * `has_api_key` and an obfuscated `api_key_hint`.
 */
export type ProviderView = {
  id: string
  label?: string
  base_url: string
  endpoint_format: string
  model: string
  enabled: boolean
  is_default: boolean
  has_api_key: boolean
  api_key_hint?: string
}

/** `GET /v1/runtime/providers` and the mutate responses share this shape. */
export type ProviderListResponse = {
  providers: ProviderView[]
  default_provider_id?: string
  active_provider_id?: string
}

/** `POST /v1/runtime/providers`. */
export type ProviderUpsertRequest = ProviderEntry & {
  make_default?: boolean
}

/** `POST /v1/runtime/providers/test` — test a stored id or an unsaved entry. */
export type ProviderTestRequest = {
  provider_id?: string
  entry?: ProviderEntry
}

/** `POST /v1/runtime/providers/test` response. */
export type ProviderTestResult = {
  ok: boolean
  status?: number
  latency_ms: number
  message: string
  request_url?: string
}

/** `GET /v1/workspace/status?path=`. */
export type WorkspaceStatus = {
  path: string
  exists: boolean
  is_git_repository: boolean
  branch?: string
  head_sha?: string
  is_dirty?: boolean
  file_change_count?: number
  checked_at: string
}

/** `POST /v1/workspace/validate`. */
export type WorkspaceValidateResult = {
  ok: true
  path: string
}

/** MCP transports the runtime can spawn. */
export const MCP_TRANSPORTS = ['stdio', 'sse', 'streamable_http'] as const
export type McpTransport = (typeof MCP_TRANSPORTS)[number]

/** One `GET /v1/mcp/servers` row. */
export type McpServerView = {
  id: string
  transport: McpTransport
  enabled: boolean
  connected: boolean
  tool_count: number
  last_error?: string
}

/** `GET /v1/mcp/servers`. */
export type McpServerListResponse = {
  servers: McpServerView[]
}

/** `POST /v1/mcp/servers`. */
export type McpServerUpsertRequest = {
  id: string
  transport: McpTransport
  enabled: boolean
  command?: string
  args?: string[]
  env?: Record<string, string>
  url?: string
  headers?: Record<string, string>
  allow_tools?: string[]
}

/** `POST /v1/mcp/servers/{id}/test`. */
export type McpTestResult = {
  ok: boolean
  tools: string[]
  latency_ms: number
  message: string
}

/** One discovered skill. */
export type SkillEntry = {
  id: string
  name: string
  description?: string
  version: string
  root: string
  entry_path: string
  priority: number
  source: string
  legacy: boolean
}

/** `GET /v1/skills`. */
export type SkillsResponse = {
  enabled: boolean
  roots: string[]
  skills: SkillEntry[]
  validation_errors: SkillValidationError[]
}

/** A skill root that failed to load; shown verbatim, never swallowed. */
export type SkillValidationError = {
  root: string
  message: string
}
