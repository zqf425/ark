/**
 * Result mapping shared by the settings hooks.
 *
 * Components own the toast copy, hooks own the call, and this module owns the
 * one place where a thrown `ApiError` becomes an operator-readable string.
 * Keeping it here means no component has to know about `ApiError`.
 */
import { ApiError } from '../../../shared/api/client'
import { translateNow } from '../../../shared/i18n/current'
import { errorHintKey } from '../../../shared/api/error-describe'

/**
 * `status` is kept so a caller can tell the documented refusals apart (409 for a
 * run already in flight) without pattern-matching the server's sentence.
 */
export type Outcome<T> = { ok: true; value: T } | { ok: false; message: string; hint?: string; status?: number }

/** `ApiError.message` is authoritative; `hint` only adds route-level context. */
export function describeApiError(error: unknown): { message: string; hint?: string; status?: number } {
  if (error instanceof ApiError) {
    const key = errorHintKey(error.code)
    // `unknown` has no taxonomy entry, so the raw status is the only honest hint.
    return { message: error.message, hint: key ? translateNow(key) : `HTTP ${error.status}`, status: error.status }
  }
  if (error instanceof Error) return { message: error.message }
  return { message: String(error) }
}

/** Run one call and fold both failure shapes into `Outcome`. */
export async function attempt<T>(run: () => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await run() }
  } catch (error) {
    const described = describeApiError(error)
    return { ok: false, message: described.message, ...(described.hint ? { hint: described.hint } : {}), ...(described.status ? { status: described.status } : {}) }
  }
}

/** One-line label for an `Outcome` failure, used in inline descriptions. */
export function describeOutcome(outcome: Outcome<unknown>): string {
  if (outcome.ok) return ''
  return outcome.hint ? `${outcome.hint}${translateNow('common.hintSeparator')}${outcome.message}` : outcome.message
}
