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

/** Mirrors the runtime clamp (`SKILLS_EXTRA_DIRS_MAX`). */
export const SKILLS_EXTRA_DIRS_MAX = 16

/** Renderer-only appearance preferences; intentionally not a Rust wire contract. */
export type UiLocale = 'zh-CN' | 'en-US'
export type UiFontSize = 'small' | 'medium' | 'large'
export type UiTheme = 'light' | 'dark' | 'system'
export type UiPreferences = { locale: UiLocale; fontSize: UiFontSize; theme: UiTheme }

/** Lifecycle of a renderer-side resource load. Pure UI state, not a wire contract. */
export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error'

/**
 * Mirrors `ProviderEntry::MODELS_MAX` (an associated const ts-rs cannot
 * export). 非 Rust 契约：数值必须与 Rust 侧保持一致，改动需双向同步。
 * The import dialog blocks before save so an oversized paste fails in place.
 */
export const PROVIDER_MODELS_MAX = 64

/**
 * Context window used when a `/models` entry publishes no usable limit.
 * The 256K default context-window budget: every
 * model row therefore gets a concrete value instead of an empty field.
 * 非 Rust 契约：纯 UI 兜底值，不进 wire 类型。
 */
export const PROVIDER_DEFAULT_CONTEXT_WINDOW = 256_000

/**
 * `GET /v1/runtime/tools` 的响应视图。非 Rust 契约：ark-server 对该路由仍返回
 * 未建模 JSON（`serde_json::Value`），这里只固化渲染所需的字段名；Rust 侧一旦
 * 建模应迁移到生成类型。
 */
export type RuntimeToolEntry = { name: string; schema: unknown }
export type RuntimeToolsReport = { version: number; names: string[]; tools: RuntimeToolEntry[] }
