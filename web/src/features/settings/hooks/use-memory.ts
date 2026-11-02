/**
 * Memory section state.
 *
 * One load per mount plus explicit refresh; every mutation reloads the list so
 * the page always shows the authoritative server projection. Mutations return
 * `{ ok, error? }` and let the component decide on toasts.
 */
import { useCallback, useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { CreateMemoryRequest, MemoryDiagnosticsResponse, MemoryEntry, UpdateMemoryRequest } from '../../../shared/contracts'
import { createMemory, deleteMemory, getMemoryDiagnostics, listMemories, updateMemory } from '../api/memory-api'
import type { LoadStatus } from '../../../shared/contracts'

export type MemoryOutcome = { ok: boolean; error?: string }

export function useMemory() {
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [entries, setEntries] = useState<MemoryEntry[]>([])
  const [diagnostics, setDiagnostics] = useState<MemoryDiagnosticsResponse | null>(null)

  const refresh = useCallback(async (): Promise<void> => {
    setStatus('loading')
    try {
      const [list, diag] = await Promise.all([listMemories(), getMemoryDiagnostics()])
      setEntries(list.entries)
      setDiagnostics(diag)
      setError(null)
      setStatus('ready')
    } catch (reason) {
      setError(describeError(reason, translateNow))
      setStatus('error')
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const create = useCallback(async (request: CreateMemoryRequest): Promise<MemoryOutcome> => {
    try {
      await createMemory(request)
      await refresh()
      return { ok: true }
    } catch (reason) {
      return { ok: false, error: describeError(reason, translateNow) }
    }
  }, [refresh])

  const update = useCallback(async (memoryId: string, request: UpdateMemoryRequest): Promise<MemoryOutcome> => {
    try {
      await updateMemory(memoryId, request)
      await refresh()
      return { ok: true }
    } catch (reason) {
      return { ok: false, error: describeError(reason, translateNow) }
    }
  }, [refresh])

  const remove = useCallback(async (memoryId: string): Promise<MemoryOutcome> => {
    try {
      await deleteMemory(memoryId)
      await refresh()
      return { ok: true }
    } catch (reason) {
      return { ok: false, error: describeError(reason, translateNow) }
    }
  }, [refresh])

  return { status, error, entries, diagnostics, refresh, create, update, remove }
}
