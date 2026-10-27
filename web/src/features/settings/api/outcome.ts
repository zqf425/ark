/**
 * Result mapping shared by the settings hooks.
 *
 * Components own the toast copy, hooks own the call, and this module owns the
 * one place where a thrown `ApiError` becomes an operator-readable string.
 * Keeping it here means no component has to know about `ApiError`.
 */
import { ApiError } from '../../../shared/api/client'

/** A write attempt: either the typed payload, or the backend's own message. */
export type Outcome<T> = { ok: true; value: T } | { ok: false; message: string; hint?: string }

/** `ApiError.message` is authoritative; `hint` only adds route-level context. */
export function describeApiError(error: unknown): { message: string; hint?: string } {
  if (error instanceof ApiError) {
    return { message: error.message, hint: error.hint }
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
    return { ok: false, message: described.message, ...(described.hint ? { hint: described.hint } : {}) }
  }
}

/** One-line label for an `Outcome` failure, used in inline descriptions. */
export function describeOutcome(outcome: Outcome<unknown>): string {
  if (outcome.ok) return ''
  return outcome.hint ? `${outcome.hint}：${outcome.message}` : outcome.message
}
