/**
 * web-only，非 Rust 契约。
 *
 * 这里仅放 Rust wire 类型未建模的浏览器请求组合、路由裸响应、运行时状态投影
 * 与界面元数据。所有可由 `ark-contract` 描述的类型均从 generated 导出。
 */
import type { Autonomy, McpServerEntry, ProviderEntry } from '../../generated'

/**
 * Autonomy values the schedule form offers, sourced from the generated union
 * so a new Rust level surfaces here as a type error. The `Exclude` alias fails
 * to compile while any generated member is missing from the array.
 */
export const SCHEDULE_AUTONOMY_VALUES = ['strict', 'workspace_write', 'unattended'] as const
type AssertAllAutonomyLevels = Exclude<
  Autonomy,
  (typeof SCHEDULE_AUTONOMY_VALUES)[number]
> extends never
  ? true
  : never
export type AutonomyLevelsAllExported = AssertAllAutonomyLevels

/** `POST /v1/runtime/providers` adds an action flag to the generated entry. */
export type ProviderUpsertRequest = ProviderEntry & { make_default?: boolean }

/** `POST /v1/mcp/servers` uses the generated server shape unchanged. */
export type McpServerUpsertRequest = McpServerEntry

/** `TurnSummary.state` is a Rust `String`; these values come from SessionStatus plus aborting. */
export type TurnState = 'idle' | 'running' | 'completed' | 'failed' | 'aborted' | 'aborting'

export const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error', 'off'] as const
export type LogLevel = (typeof LOG_LEVELS)[number]

export const ENDPOINT_FORMATS = ['openai_chat', 'openai_responses', 'anthropic_messages'] as const
export type EndpointFormat = (typeof ENDPOINT_FORMATS)[number]

export const SUBAGENT_MIN_CONCURRENCY = 1
export const SUBAGENT_MAX_CONCURRENCY = 8

/** Renderer-only appearance preferences; intentionally not a Rust wire contract. */
export type UiLocale = 'zh-CN' | 'en-US'
export type UiFontSize = 'small' | 'medium' | 'large'
export type UiTheme = 'light' | 'dark' | 'system'
export type UiPreferences = { locale: UiLocale; fontSize: UiFontSize; theme: UiTheme }

/** Lifecycle of a renderer-side resource load. Pure UI state, not a wire contract. */
export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error'
