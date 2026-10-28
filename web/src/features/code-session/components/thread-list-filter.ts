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
