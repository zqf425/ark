/**
 * Scheduled-task lifecycle for the settings page.
 *
 * Every write answers with the single task it touched (`POST`/`PATCH` return the
 * stored task, `POST /run` returns it with the `running` marker), so the hook
 * folds that answer straight back into the list instead of re-fetching. The
 * master switch lives in runtime config and is therefore the caller's business
 * (`SubagentSection` pattern), not this hook's.
 */
import { useCallback, useEffect, useState } from 'react'
import type {
  LoadStatus, CreateScheduleRequest, ScheduledTask, UpdateScheduleRequest } from '../../../shared/contracts'
import {
  createSchedule,
  deleteSchedule,
  listSchedules,
  runScheduleNow,
  updateSchedule
} from '../api/schedules-api'
import { useMounted } from '../../../shared/api/use-mounted'
import { attempt, describeApiError, type Outcome } from '../api/outcome'

export type SchedulesApi = {
  tasks: ScheduledTask[]
  status: LoadStatus
  error: string | null
  /** Non-null while a mutation runs; the value names the row being changed. */
  busy: string | null
  refresh: () => Promise<void>
  create: (request: CreateScheduleRequest) => Promise<Outcome<ScheduledTask>>
  update: (taskId: string, request: UpdateScheduleRequest) => Promise<Outcome<ScheduledTask>>
  remove: (taskId: string) => Promise<Outcome<void>>
  runNow: (taskId: string) => Promise<Outcome<ScheduledTask>>
}

export function useSchedules(): SchedulesApi {
  const [tasks, setTasks] = useState<ScheduledTask[]>([])
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const mounted = useMounted()

  /** Replace one row in place: the store owns ordering, so no re-sort here. */
  const apply = useCallback((task: ScheduledTask) => {
    setTasks((current) => {
      const index = current.findIndex((row) => row.id === task.id)
      if (index === -1) return [task, ...current]
      const next = [...current]
      next[index] = task
      return next
    })
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const response = await listSchedules()
      if (!mounted.current) return
      setTasks(response.tasks ?? [])
      setError(null)
      setStatus('ready')
    } catch (thrown) {
      if (!mounted.current) return
      setError(describeApiError(thrown).message)
      setStatus('error')
    }
  }, [])

  const mutate = useCallback(
    async <T,>(key: string, run: () => Promise<T>): Promise<Outcome<T>> => {
      setBusy(key)
      const outcome = await attempt(run)
      if (mounted.current) setBusy(null)
      return outcome
    },
    []
  )

  const create = useCallback(
    async (request: CreateScheduleRequest): Promise<Outcome<ScheduledTask>> => {
      const outcome = await mutate('create', () => createSchedule(request))
      if (outcome.ok && mounted.current) {
        apply(outcome.value)
        setStatus('ready')
      }
      return outcome
    },
    [apply, mutate]
  )

  const update = useCallback(
    async (taskId: string, request: UpdateScheduleRequest): Promise<Outcome<ScheduledTask>> => {
      const outcome = await mutate(`update:${taskId}`, () => updateSchedule(taskId, request))
      if (outcome.ok && mounted.current) apply(outcome.value)
      return outcome
    },
    [apply, mutate]
  )

  const remove = useCallback(
    async (taskId: string): Promise<Outcome<void>> => {
      const outcome = await mutate(`delete:${taskId}`, () => deleteSchedule(taskId))
      if (outcome.ok && mounted.current) {
        setTasks((current) => current.filter((row) => row.id !== taskId))
      }
      return outcome
    },
    [mutate]
  )

  const runNow = useCallback(
    async (taskId: string): Promise<Outcome<ScheduledTask>> => {
      const outcome = await mutate(`run:${taskId}`, () => runScheduleNow(taskId))
      if (outcome.ok && mounted.current) apply(outcome.value)
      return outcome
    },
    [apply, mutate]
  )

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { tasks, status, error, busy, refresh, create, update, remove, runNow }
}
