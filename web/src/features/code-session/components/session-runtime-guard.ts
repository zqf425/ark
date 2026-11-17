/**
 * Ownership rules for the per-thread runtime form.
 *
 * The runtime pills show one thread's stored overrides while their data is
 * per-thread, so "which thread are these values from?" has to be an explicit
 * fact rather than an assumption. Kept pure so the regression is testable
 * without a DOM.
 */

/** The thread whose stored overrides are currently loaded; `null` = not loaded. */
export type RuntimeOwner = string | null

/**
 * A save may only target the thread the form was loaded for. An unloaded or
 * failed load (`null`) is not permission to write: it would push the empty
 * defaults of one thread over another thread's stored overrides.
 */
export function canSaveRuntime(loadedFor: RuntimeOwner, threadId: string): boolean {
  return loadedFor !== null && loadedFor === threadId
}
