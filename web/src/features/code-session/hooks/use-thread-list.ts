/**
 * Thread list lifecycle: bootstrap, refresh, create.
 *
 * All runtime access goes through `../api/threads-api`; the hook only decides
 * *when* to call and how to reflect the result in the store.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { ThreadSummary } from '../../../shared/contracts'
import { createThread, listThreads } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  if (error instanceof Error) return error.message
  return String(error)
}

export type ThreadListApi = {
  refresh: () => Promise<void>
  create: () => Promise<ThreadSummary | null>
  select: (threadId: string | null) => void
  busy: boolean
}

export function useThreadList(): ThreadListApi {
  const threads = useCodeSessionStore((s) => s.threads)
  const listStatus = useCodeSessionStore((s) => s.listStatus)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const setThreads = useCodeSessionStore((s) => s.setThreads)
  const setListStatus = useCodeSessionStore((s) => s.setListStatus)
  const upsertThread = useCodeSessionStore((s) => s.upsertThread)
  const setActiveThread = useCodeSessionStore((s) => s.setActiveThread)
  const [busy, setBusy] = useState(false)
  const mounted = useRef(true)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])

  const refresh = useCallback(async () => {
    setListStatus('loading')
    try {
      const rows = await listThreads()
      if (!mounted.current) return
      setThreads(rows.filter((row) => !row.archived))
    } catch (error) {
      if (mounted.current) setListStatus('error', describe(error))
    }
  }, [setListStatus, setThreads])

  const create = useCallback(async () => {
    setBusy(true)
    try {
      const created = await createThread({})
      if (!mounted.current) return null
      upsertThread(created)
      setActiveThread(created.thread_id)
      return created
    } catch (error) {
      if (mounted.current) setListStatus('error', describe(error))
      return null
    } finally {
      if (mounted.current) setBusy(false)
    }
  }, [setActiveThread, setListStatus, upsertThread])

  // Select the newest thread once the first load lands, so the screen is never
  // an empty shell when history exists.
  useEffect(() => {
    if (listStatus !== 'ready') return
    if (activeThreadId) return
    const first = threads[0]
    if (first) setActiveThread(first.thread_id)
  }, [activeThreadId, listStatus, setActiveThread, threads])

  return { refresh, create, select: setActiveThread, busy }
}
