/**
 * One fetch for the three facts the console chrome needs: active workspace
 * path, Git head, changed-file count and the model the loop is bound to.
 *
 * Kept in one hook so the top bar and the status bar never disagree and the
 * shell issues exactly one round of requests per refresh.
 */
import { useCallback, useEffect, useState } from 'react'
import { getWorkspaceDiff, getWorkspaceView } from '../../settings/api/workspace-api'
import { listProviders } from '../../settings/api/providers-api'

export type ConsoleMeta = {
  workspacePath: string | null
  headSha: string | null
  changeCount: number
  modelLabel: string | null
  refresh: () => void
}

const EMPTY: Omit<ConsoleMeta, 'refresh'> = {
  workspacePath: null,
  headSha: null,
  changeCount: 0,
  modelLabel: null
}

export function useConsoleMeta(): ConsoleMeta {
  const [meta, setMeta] = useState<Omit<ConsoleMeta, 'refresh'>>(EMPTY)
  const [tick, setTick] = useState(0)

  useEffect(() => {
    const controller = new AbortController()
    void Promise.all([
      getWorkspaceView(controller.signal).catch(() => null),
      getWorkspaceDiff(controller.signal).catch(() => null),
      listProviders(controller.signal).catch(() => null)
    ]).then(([view, diff, providers]) => {
      if (controller.signal.aborted) return
      const activeId = providers?.active_provider_id ?? providers?.default_provider_id
      const active = providers?.providers.find((provider) => provider.id === activeId)
      setMeta({
        workspacePath: view?.active?.path ?? null,
        headSha: diff?.head_sha ?? null,
        changeCount: diff?.file_count ?? 0,
        modelLabel: active?.model ?? null
      })
    })
    return () => controller.abort()
  }, [tick])

  const refresh = useCallback(() => setTick((value) => value + 1), [])
  return { ...meta, refresh }
}