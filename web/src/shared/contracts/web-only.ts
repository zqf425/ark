/**
 * web-only，非 Rust 契约。
 *
 * 这里仅放 Rust wire 类型未建模的浏览器请求组合、路由裸响应、运行时状态投影
 * 与界面元数据。所有可由 `ark-contract` 描述的类型均从 generated 导出。
 */
import type { ArkErrorCode, McpServerEntry, ProviderEntry } from '../../generated'

/** `POST /v1/runtime/providers` adds an action flag to the generated entry. */
export type ProviderUpsertRequest = ProviderEntry & { make_default?: boolean }

/** `POST /v1/mcp/servers` uses the generated server shape unchanged. */
export type McpServerUpsertRequest = McpServerEntry

/** `POST /v1/workspace/validate` currently returns an untyped JSON object. */
export type WorkspaceValidateResult = { ok: true; path: string }

/** `TurnSummary.state` is a Rust `String`; these values come from SessionStatus plus aborting. */
export type TurnState = 'idle' | 'running' | 'completed' | 'failed' | 'aborted' | 'aborting'

/** Human-facing text is UI metadata, not a wire contract. */
export const ERROR_HINTS: Record<ArkErrorCode, string> = {
  validation_error: '请求参数不合法',
  not_found: '目标不存在或已被删除',
  unauthorized: '缺少或无效的 ark_token',
  conflict: '运行时状态冲突，请稍后重试',
  model_request_failed: '模型请求失败，请检查 provider 配置',
  tool_execution_failed: '工具执行失败',
  runtime_unavailable: '运行时不可用',
  store_failure: '会话存储失败',
  not_implemented: '运行时尚未实现该能力',
  internal_error: '运行时内部错误'
}

export const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error', 'off'] as const
export type LogLevel = (typeof LOG_LEVELS)[number]

export const ENDPOINT_FORMATS = ['openai_chat', 'openai_responses', 'anthropic_messages'] as const
export type EndpointFormat = (typeof ENDPOINT_FORMATS)[number]

export const MCP_TRANSPORTS = ['stdio', 'sse', 'streamable_http'] as const
export const SUBAGENT_MIN_CONCURRENCY = 1
export const SUBAGENT_MAX_CONCURRENCY = 8
