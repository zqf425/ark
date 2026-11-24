/**
 * The thread's child-run list, shared by the in-canvas background banner and
 * the composer hint (Qoder-referenced session layout).
 *
 * Module-level cache keyed by thread id + `childRunRevision`, the same shape
 * as `use-thread-skills.ts`: concurrent mounts share one request, and the
 * list is re-read only when `child_activity`/`child_completed` bumps the
 * revision — never per streamed token. A failed fetch is not cached as fresh
 * data; the previous snapshot (if any) keeps rendering until a read lands.
 */
import { useEffect, useState } from 'react'
import type { ChildRunSummary } from '../../../shared/contracts'
import { listChildRuns } from '../api/child-runs-api'
import { useCodeSessionStore } from '../state/store'

type CacheEntry = { revision: number; runs: ChildRunSummary[] | null; promise?: Promise<void> }

const cache = new Map<string, CacheEntry>()

/** Runs that still hold (or wait for) a concurrency slot. */
export function isActiveChildRun(run: ChildRunSummary): boolean {
  return run.status === 'running' || run.status === 'queued'
}

export function useChildActivity(threadId: string | null): ChildRunSummary[] | null {
  const revision = useCodeSessionStore((s) => s.childRunRevision)
  const [, setTick] = useState(0)

  useEffect(() => {
    if (!threadId) return undefined
    const existing = cache.get(threadId)
    if (existing && existing.revision === revision && existing.runs && !existing.promise) return undefined
    const entry: CacheEntry =
      existing && existing.revision === revision ? existing : { revision, runs: existing?.runs ?? null }
    if (!entry.promise) {
      entry.promise = listChildRuns(threadId)
        .then((response) => {
          entry.runs = response.child_runs
        })
        .catch(() => {})
        .finally(() => {
          entry.promise = undefined
          // A newer revision may have replaced the entry while this read was
          // in flight; the stale response must not clobber it.
          if (cache.get(threadId) === entry || !cache.has(threadId)) cache.set(threadId, entry)
          setTick((tick) => tick + 1)
        })
      cache.set(threadId, entry)
    }
    void entry.promise.then(() => setTick((tick) => tick + 1))
    return undefined
  }, [threadId, revision])

  const entry = threadId ? cache.get(threadId) : undefined
  return entry?.runs ?? null
}