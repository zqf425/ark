import type { ThreadSummary } from '../../../shared/contracts'

/** UI-only filter over the already loaded thread list; it never triggers a search request. */
export type ThreadListScope = 'recent' | 'archived'

export function filterThreads(
  threads: readonly ThreadSummary[],
  query: string,
  scope: ThreadListScope
): ThreadSummary[] {
  const needle = query.trim().toLocaleLowerCase()

  return threads.filter((thread) => {
    if (scope === 'archived' ? !thread.archived : thread.archived) return false
    return !needle || thread.title.toLocaleLowerCase().includes(needle)
  })
}

/**
 * Project-aware filter for the grouped rail: a row survives when its scope
 * matches and **either** its title or its workspace hits the needle, so
 * searching "ark" also surfaces every thread of the project whose folder is
 * named ark.
 */
export function filterThreadsWithWorkspace(
  threads: readonly ThreadSummary[],
  query: string,
  scope: ThreadListScope
): ThreadSummary[] {
  const needle = query.trim().toLocaleLowerCase()
  if (!needle) return filterThreads(threads, '', scope)
  return threads.filter((thread) => {
    if (scope === 'archived' ? !thread.archived : thread.archived) return false
    const inTitle = thread.title.toLocaleLowerCase().includes(needle)
    const inWorkspace = (thread.workspace ?? '').toLocaleLowerCase().includes(needle)
    return inTitle || inWorkspace
  })
}
