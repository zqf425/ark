/**
 * Client-produced error copy.
 *
 * The runtime's own `message` is authoritative and stays in whatever language
 * the runtime produced; only the short hint that explains the error *code* is
 * client copy and therefore localized here. `describeError` is the one place
 * that joins the two, so every caller gets the same separator and the same
 * fallback instead of re-implementing `hint：message`.
 */
import type { ArkErrorCode } from '../contracts'
import type { MessageKey, MessageParams } from '../i18n/messages'
import { ApiError } from './client'

/** Minimal translator shape so this module never imports React. */
export type Translator = (key: MessageKey, params?: MessageParams) => string

const HINT_KEYS: Record<ArkErrorCode, MessageKey> = {
  validation_error: 'error.hint.validation_error',
  not_found: 'error.hint.not_found',
  unauthorized: 'error.hint.unauthorized',
  conflict: 'error.hint.conflict',
  model_request_failed: 'error.hint.model_request_failed',
  tool_execution_failed: 'error.hint.tool_execution_failed',
  runtime_unavailable: 'error.hint.runtime_unavailable',
  store_failure: 'error.hint.store_failure',
  not_implemented: 'error.hint.not_implemented',
  internal_error: 'error.hint.internal_error'
}

/** `null` means "no taxonomy entry": the caller falls back to the raw status. */
export function errorHintKey(code: ArkErrorCode | 'unknown'): MessageKey | null {
  return code === 'unknown' ? null : HINT_KEYS[code]
}

/**
 * One error, one line: localized hint first, then the server's own message.
 * `unknown` keeps the pre-i18n behaviour of reporting the HTTP status.
 */
export function describeError(error: unknown, t: Translator): string {
  if (error instanceof ApiError) {
    const key = errorHintKey(error.code)
    if (!key) return `HTTP ${error.status}${t('common.hintSeparator')}${error.message}`
    return `${t(key)}${t('common.hintSeparator')}${error.message}`
  }
  if (error instanceof Error) return error.message
  return String(error)
}
