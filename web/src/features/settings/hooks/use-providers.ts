/**
 * Provider list lifecycle (v1 routes).
 *
 * Every mutate endpoint answers with the full list, so this hook folds the
 * response straight back into state instead of re-fetching. `active` is the
 * provider the loop is actually using; `default` is only the configured one.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  ProviderListResponse,
  ProviderTestRequest,
  ProviderTestResponse,
  ProviderUpsertRequest,
  ProviderView
} from '../../../shared/contracts'
import { deleteProvider, listProviders, setDefaultProvider, testProvider, upsertProvider } from '../api/providers-api'
import { attempt, describeApiError, type Outcome } from '../api/outcome'
import type { LoadStatus } from './use-settings'

export type ProvidersApi = {
  providers: ProviderView[]
  defaultProviderId: string | null
  activeProviderId: string | null
  status: LoadStatus
  error: string | null
  /** Non-null while a mutation runs; the value names the row being changed. */
  busy: string | null
  refresh: () => Promise<void>
  upsert: (request: ProviderUpsertRequest) => Promise<Outcome<ProviderListResponse>>
  remove: (providerId: string) => Promise<Outcome<ProviderListResponse>>
  setDefault: (providerId: string) => Promise<Outcome<ProviderListResponse>>
  test: (request: ProviderTestRequest) => Promise<Outcome<ProviderTestResponse>>
}

export function useProviders(): ProvidersApi {
  const [providers, setProviders] = useState<ProviderView[]>([])
  const [defaultProviderId, setDefaultProviderId] = useState<string | null>(null)
  const [activeProviderId, setActiveProviderId] = useState<string | null>(null)
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const apply = useCallback((response: ProviderListResponse) => {
    setProviders(response.providers ?? [])
    setDefaultProviderId(response.default_provider_id ?? null)
    setActiveProviderId(response.active_provider_id ?? null)
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const response = await listProviders()
      if (!mounted.current) return
      apply(response)
      setError(null)
      setStatus('ready')
    } catch (thrown) {
      if (!mounted.current) return
      setError(describeApiError(thrown).message)
      setStatus('error')
    }
  }, [apply])

  const mutate = useCallback(
    async (key: string, run: () => Promise<ProviderListResponse>): Promise<Outcome<ProviderListResponse>> => {
      setBusy(key)
      const outcome = await attempt(run)
      if (mounted.current) {
        setBusy(null)
        if (outcome.ok) {
          apply(outcome.value)
          setStatus('ready')
        }
      }
      return outcome
    },
    [apply]
  )

  const upsert = useCallback(
    (request: ProviderUpsertRequest) => mutate(`save:${request.id}`, () => upsertProvider(request)),
    [mutate]
  )
  const remove = useCallback(
    (providerId: string) => mutate(`delete:${providerId}`, () => deleteProvider(providerId)),
    [mutate]
  )
  const setDefault = useCallback(
    (providerId: string) => mutate(`default:${providerId}`, () => setDefaultProvider(providerId)),
    [mutate]
  )
  const test = useCallback(async (request: ProviderTestRequest) => {
    setBusy(`test:${request.provider_id ?? request.entry?.id ?? 'draft'}`)
    const outcome = await attempt(() => testProvider(request))
    if (mounted.current) setBusy(null)
    return outcome
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { providers, defaultProviderId, activeProviderId, status, error, busy, refresh, upsert, remove, setDefault, test }
}
