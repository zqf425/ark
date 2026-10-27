/**
 * 待由 ark-contract 生成，CI 校验 diff 为空。
 * Mirrors `crates/ark-common/src/error.rs` (design §6.2).
 */
export type ArkErrorCode =
  | 'validation_error'
  | 'not_found'
  | 'unauthorized'
  | 'conflict'
  | 'model_request_failed'
  | 'tool_execution_failed'
  | 'runtime_unavailable'
  | 'store_failure'
  | 'not_implemented'
  | 'internal_error'

/** Wire shape of an HTTP error body and of the SSE `error` event. */
export type ArkErrorPayload = {
  code: ArkErrorCode
  message: string
  detail?: string
}

/** Human-facing hint per error code; keeps code→copy mapping in one place. */
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
