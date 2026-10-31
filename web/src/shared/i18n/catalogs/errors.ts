/**
 * Client-produced error hints, keyed by the runtime error taxonomy.
 *
 * The hint is client copy (it explains a code the runtime already chose), while
 * `ApiError.message` stays the server's own sentence in whatever language the
 * runtime produced. The two are joined by `common.hintSeparator`.
 */
export const ERROR_MESSAGES = {
  'error.hint.validation_error': ['请求参数不合法', 'Invalid request parameters'],
  'error.hint.not_found': ['目标不存在或已被删除', 'The target does not exist or was deleted'],
  'error.hint.unauthorized': ['缺少或无效的 ark_token', 'Missing or invalid ark_token'],
  'error.hint.conflict': ['运行时状态冲突，请稍后重试', 'Runtime state conflict; try again shortly'],
  'error.hint.model_request_failed': ['模型请求失败，请检查 provider 配置', 'Model request failed; check the provider configuration'],
  'error.hint.tool_execution_failed': ['工具执行失败', 'Tool execution failed'],
  'error.hint.runtime_unavailable': ['运行时不可用', 'The runtime is unavailable'],
  'error.hint.store_failure': ['会话存储失败', 'Session storage failed'],
  'error.hint.not_implemented': ['运行时尚未实现该能力', 'The runtime does not implement this yet'],
  'error.hint.internal_error': ['运行时内部错误', 'Internal runtime error']
} as const
