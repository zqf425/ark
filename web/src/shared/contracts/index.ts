/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 *
 * Barrel for the wire contract. Everything under this folder is generated from
 * `crates/ark-contract`; a hand edit here is a CI failure by design.
 */
export type { ThreadId, TurnId, ItemId, ChildId } from './ids'
export type { ItemKind, ItemStatus, ToolPayload, ThreadItem } from './item'
export { isItemLive } from './item'
export type {
  EventKind,
  PassiveEventKind,
  RuntimeEvent,
  RuntimeEventBase,
  PassiveRuntimeEvent
} from './event'
export type {
  Usage,
  HealthResponse,
  RuntimeInfo,
  ThreadSummary,
  CreateThreadRequest,
  UpdateThreadRequest,
  ThreadDetail,
  TurnRequest,
  TurnState,
  TurnSummary,
  UsageResponse
} from './dto'
export type { ArkErrorCode, ArkErrorPayload } from './error'
export { ERROR_HINTS } from './error'
export type {
  RuntimeConfig,
  SubagentConfig,
  ConfigPatch,
  ConfigApplyResult,
  LogLevel,
  EndpointFormat,
  ProviderEntry,
  ProviderView,
  ProviderListResponse,
  ProviderUpsertRequest,
  ProviderTestRequest,
  ProviderTestResult,
  WorkspaceStatus,
  WorkspaceValidateResult,
  McpTransport,
  McpServerView,
  McpServerListResponse,
  McpServerUpsertRequest,
  McpTestResult,
  SkillEntry,
  SkillsResponse,
  SkillValidationError
} from './settings'
export { LOG_LEVELS, ENDPOINT_FORMATS, MCP_TRANSPORTS, SUBAGENT_MIN_CONCURRENCY, SUBAGENT_MAX_CONCURRENCY } from './settings'
