/**
 * MCP server lifecycle (v1 routes).
 *
 * `test` is a probe, so its result stays with the caller and is shown per row;
 * only list-changing calls are folded back into state.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type { McpServerListResponse, McpServerUpsertRequest, McpServerView, McpServerTestResponse } from '../../../shared/contracts'
import { deleteMcpServer, listMcpServers, testMcpServer, upsertMcpServer } from '../api/mcp-api'
import { attempt, describeApiError, type Outcome } from '../api/outcome'
import type { LoadStatus } from './use-settings'

export type McpApi = {
  servers: McpServerView[]
  status: LoadStatus
  error: string | null
  busy: string | null
  refresh: () => Promise<void>
  add: (request: McpServerUpsertRequest) => Promise<Outcome<McpServerListResponse>>
  remove: (serverId: string) => Promise<Outcome<McpServerListResponse>>
  test: (serverId: string) => Promise<Outcome<McpServerTestResponse>>
}

export function useMcpServers(): McpApi {
  const [servers, setServers] = useState<McpServerView[]>([])
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

  const apply = useCallback((response: McpServerListResponse) => {
    setServers(response.servers ?? [])
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const response = await listMcpServers()
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
    async (key: string, run: () => Promise<McpServerListResponse>): Promise<Outcome<McpServerListResponse>> => {
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

  const add = useCallback((request: McpServerUpsertRequest) => mutate(`save:${request.id}`, () => upsertMcpServer(request)), [mutate])
  const remove = useCallback((serverId: string) => mutate(`delete:${serverId}`, () => deleteMcpServer(serverId)), [mutate])

  const test = useCallback(async (serverId: string) => {
    setBusy(`test:${serverId}`)
    const outcome = await attempt(() => testMcpServer(serverId))
    if (mounted.current) setBusy(null)
    return outcome
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { servers, status, error, busy, refresh, add, remove, test }
}
