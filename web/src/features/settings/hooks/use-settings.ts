/**
 * Runtime config lifecycle for the settings page.
 *
 * Loads `GET /v1/runtime/config` once, and exposes one `save` that maps a
 * `ConfigPatch` onto `POST /v1/runtime/config`. A `rejected` answer is a normal
 * result (not an error) so the sections can show `restart_required` verbatim.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type { ApplyResult, ConfigPatch, RuntimeConfigView, WorkspaceStatus, WorkspaceValidateResponse } from '../../../shared/contracts'
import { getRuntimeConfig, patchRuntimeConfig } from '../api/settings-api'
import { getWorkspaceStatus, validateWorkspace } from '../api/workspace-api'
import { attempt, describeApiError, type Outcome } from '../api/outcome'
import { isLatestWorkspaceProbe } from './workspace-probe-guard'

export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error'

export type SettingsApi = {
  config: RuntimeConfigView | null
  status: LoadStatus
  error: string | null
  /** Key of the patch currently in flight (`log_level`, `default_workspace`, ...). */
  saving: string | null
  refresh: () => Promise<void>
  save: (patch: ConfigPatch, key: string) => Promise<Outcome<ApplyResult>>
}

export function useSettings(): SettingsApi {
  const [config, setConfig] = useState<RuntimeConfigView | null>(null)
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState<string | null>(null)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const next = await getRuntimeConfig()
      if (!mounted.current) return
      setConfig(next)
      setError(null)
      setStatus('ready')
    } catch (thrown) {
      if (!mounted.current) return
      setError(describeApiError(thrown).message)
      setStatus('error')
    }
  }, [])

  const save = useCallback(
    async (patch: ConfigPatch, key: string): Promise<Outcome<ApplyResult>> => {
      setSaving(key)
      const outcome = await attempt(() => patchRuntimeConfig(patch))
      if (mounted.current) {
        setSaving(null)
        // Re-read so `version`, `log_level` and `default_workspace` reflect the
        // runtime's normalisation rather than the local optimistic guess.
        if (outcome.ok && outcome.value.result === 'applied') void refresh()
      }
      return outcome
    },
    [refresh]
  )

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { config, status, error, saving, refresh, save }
}

export type WorkspaceProbeApi = {
  status: WorkspaceStatus | null
  error: string | null
  loading: boolean
  probe: (path: string) => Promise<void>
  validate: (path: string) => Promise<Outcome<WorkspaceValidateResponse>>
}

/**
 * Filesystem probe for the default workspace field.
 *
 * `validate` is the runtime's own path check; the browser never guesses whether
 * a path exists. An empty path clears the local probe instead of calling out.
 */
export function useWorkspaceProbe(): WorkspaceProbeApi {
  const [status, setStatus] = useState<WorkspaceStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const mounted = useRef(true)
  const probeSequence = useRef(0)
  const activeProbe = useRef<AbortController | null>(null)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      activeProbe.current?.abort()
    }
  }, [])

  const probe = useCallback(async (path: string) => {
    const trimmed = path.trim()
    activeProbe.current?.abort()
    const request = probeSequence.current + 1
    probeSequence.current = request
    if (!trimmed) {
      setStatus(null)
      setError(null)
      setLoading(false)
      return
    }
    const controller = new AbortController()
    activeProbe.current = controller
    setLoading(true)
    try {
      const next = await getWorkspaceStatus(trimmed, controller.signal)
      if (!mounted.current || controller.signal.aborted || !isLatestWorkspaceProbe(probeSequence.current, request)) return
      setStatus(next)
      setError(null)
    } catch (thrown) {
      if (!mounted.current || controller.signal.aborted || !isLatestWorkspaceProbe(probeSequence.current, request)) return
      setStatus(null)
      setError(describeApiError(thrown).message)
    } finally {
      if (mounted.current && isLatestWorkspaceProbe(probeSequence.current, request)) setLoading(false)
    }
  }, [])

  const validate = useCallback(async (path: string) => {
    const outcome = await attempt(() => validateWorkspace(path.trim()))
    if (outcome.ok && mounted.current) {
      // A successful validation is the cheapest moment to refresh the probe.
      void probe(path)
    }
    return outcome
  }, [probe])

  return { status, error, loading, probe, validate }
}
