/**
 * Ownership bookkeeping for the plan panel's local copy of one thread's plan.
 *
 * The panel stays mounted across thread switches while all of its data is
 * per-thread, so "which thread is this plan from?" has to be an explicit fact
 * instead of an assumption. These helpers are pure so the regression is
 * testable without a DOM.
 */

export type PlanOwnership = {
  /** Thread whose plan, goal and todos are on screen; `null` while none is shown. */
  owner: string | null
  /** Goal the server last returned: the baseline an "unedited draft" compares to. */
  serverGoal: string
  /** Newest request generation. A response from an older one must be dropped. */
  generation: number
}

/**
 * A write may only target the thread that owns what is currently displayed.
 * A stale owner (A's plan still on screen after switching to B) is dropped
 * instead of being written into B.
 */
export function isPlanOwner(owner: string | null, threadId: string | null): boolean {
  return owner !== null && owner === threadId
}

/**
 * A thread switch starts from an empty panel and invalidates in-flight work:
 * the bumped generation is what stops a late `getPlan` or a late save from the
 * previous thread from writing into the new one.
 */
export function resetPlanOwnership(threadId: string | null, generation: number): PlanOwnership {
  return { owner: threadId, serverGoal: '', generation: generation + 1 }
}

/**
 * Starting a load makes the target thread the owner up front: on the first mount
 * there is no previous owner, and a response guard that only accepts an already
 * matching owner would otherwise drop the very first `getPlan` forever. The
 * server-goal baseline is kept only when it belongs to the same thread, because
 * that baseline is what decides whether the draft is an untouched copy or a
 * user edit.
 */
export function beginPlanFetch(previous: PlanOwnership, threadId: string): PlanOwnership {
  const sameThread = previous.owner === threadId
  return {
    owner: threadId,
    serverGoal: sameThread ? previous.serverGoal : '',
    generation: previous.generation + 1
  }
}

/**
 * A response may paint only while the panel still targets its thread and no
 * later request or save has taken over. This is the ordering guarantee between
 * an event-driven reload and a save: whichever started last wins.
 */
export function shouldApplyPlanResponse(ownership: PlanOwnership, threadId: string, generation: number): boolean {
  return ownership.owner === threadId && isLatestPlanRequest(ownership.generation, generation)
}

/**
 * Adopt a refreshed server goal only while the textarea still holds the value
 * the server last returned. Anything else is a user edit and wins until saved,
 * so an event-driven reload never overwrites a draft in progress.
 */
export function adoptServerGoal(draft: string, lastServerGoal: string, nextServerGoal: string): string {
  return draft === lastServerGoal ? nextServerGoal : draft
}

/** A response may update the panel only when it belongs to the newest request. */
export function isLatestPlanRequest(latest: number, candidate: number): boolean {
  return latest === candidate
}
