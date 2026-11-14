/**
 * The thread's loaded skill set (base + extra), shared by every surface that
 * renders skill mentions: the composer's palette and the message canvas.
 *
 * Module-level cache keyed by thread id: the response is read-only discovery
 * state, so concurrent mounts (composer open + canvas rows rendering) share one
 * request instead of fanning out. `force` bypasses the cache for the rescan
 * path. A failed fetch is not cached — the next mount retries.
 *
 * `threadId = null` reads the cache without fetching: mention chips only need
 * the set the composer already loaded for this thread, and must never turn a
 * historical message row into an API caller. If nothing is cached yet the
 * consumer renders plain text, which is the pre-feature behavior.
 */
import { useEffect, useState } from 'react'
import type { SkillSummary } from '../../../shared/contracts'
import { listSkills, listThreadSkills, refreshThreadSkills } from '../api/skills-api'

const CACHE_TTL_MS = 30_000

type CacheEntry = { skills: SkillSummary[] | null; warnings: number; at: number; promise?: Promise<void> }

const cache = new Map<string, CacheEntry>()

function readFresh(entry: CacheEntry | undefined): CacheEntry | null {
  return entry && entry.skills && Date.now() - entry.at < CACHE_TTL_MS ? entry : null
}

async function load(threadId: string, entry: CacheEntry, force: boolean): Promise<void> {
  const view = force
    ? await refreshThreadSkills(threadId)
    : await listThreadSkills(threadId)
  entry.skills = [...view.base_skills, ...view.extra_skills]
  entry.warnings = view.warnings.length
  entry.at = Date.now()
}

/**
 * The cached read. `skills` stays `null` while loading (and on failure), so
 * callers keep their existing loading/empty rendering.
 */
export function useThreadSkills(threadId: string | null): { skills: SkillSummary[] | null; warnings: number; reload: () => void } {
  const [version, setVersion] = useState(0)
  const [, setTick] = useState(0)

  useEffect(() => {
    if (!threadId) return
    const entry = cache.get(threadId) ?? { skills: null, warnings: 0, at: 0 }
    if (readFresh(entry)) return
    if (!entry.promise) {
      entry.promise = load(threadId, entry, false)
        .catch(() => {})
        .finally(() => {
          entry.promise = undefined
          cache.set(threadId, entry)
          setTick((tick) => tick + 1)
        })
      cache.set(threadId, entry)
    }
    // Re-render when the shared promise lands (another subscriber may have
    // started it); the promise identity is intentionally not a dependency.
    void entry.promise.then(() => setTick((tick) => tick + 1))
    return undefined
  }, [threadId, version])

  const entry = threadId ? cache.get(threadId) : undefined
  const reload = (): void => {
    if (!threadId) return
    const current = cache.get(threadId) ?? { skills: null, warnings: 0, at: 0 }
    void refreshThreadSkills(threadId)
      .then((view) => {
        current.skills = [...view.base_skills, ...view.extra_skills]
        current.warnings = view.warnings.length
        current.at = Date.now()
        cache.set(threadId, current)
      })
      .catch(() => {})
      .finally(() => setVersion((value) => value + 1))
  }

  return { skills: entry?.skills ?? null, warnings: entry?.warnings ?? 0, reload }
}

/** Runtime-wide fallback for the no-thread state (composer before a session). */
export function useRuntimeSkills(): SkillSummary[] | null {
  const [skills, setSkills] = useState<SkillSummary[] | null>(null)
  useEffect(() => {
    let cancelled = false
    void listSkills()
      .then((diagnostics) => {
        if (!cancelled) setSkills(diagnostics.skills)
      })
      .catch(() => {
        if (!cancelled) setSkills([])
      })
    return () => {
      cancelled = true
    }
  }, [])
  return skills
}
