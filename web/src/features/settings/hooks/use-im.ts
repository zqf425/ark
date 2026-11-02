/**
 * IM channel lifecycle for the settings page.
 *
 * Writes answer with the touched channel view, so the hook folds it back into
 * the list instead of re-fetching. The master switch lives in runtime config
 * (the caller's business, `ScheduleSection` pattern).
 */
import { useCallback, useEffect, useState } from 'react'
import type {
  CreateImChannelRequest,
  ImChannelListItem,
  ImChannelView,
  ImWeixinLoginWaitResponse,
  LoadStatus,
  UpdateImChannelRequest
} from '../../../shared/contracts'
import {
  createImChannel,
  deleteImChannel,
  listImChannels,
  startWeixinLogin,
  updateImChannel,
  verifyImChannel,
  waitWeixinLogin
} from '../api/im-api'
import { useMounted } from '../../../shared/api/use-mounted'
import { attempt, describeApiError, type Outcome } from '../api/outcome'

export type ImApi = {
  channels: ImChannelListItem[]
  status: LoadStatus
  error: string | null
  busy: string | null
  refresh: () => Promise<void>
  create: (request: CreateImChannelRequest) => Promise<Outcome<ImChannelView>>
  update: (channelId: string, request: UpdateImChannelRequest) => Promise<Outcome<ImChannelView>>
  remove: (channelId: string) => Promise<Outcome<void>>
  verify: (channelId: string) => Promise<Outcome<string>>
  loginStart: () => Promise<Outcome<{ sessionId: string; qrContent: string }>>
  loginWait: (sessionId: string) => Promise<Outcome<ImWeixinLoginWaitResponse>>
}

export function useIm(): ImApi {
  const [channels, setChannels] = useState<ImChannelListItem[]>([])
  const [status, setStatus] = useState<LoadStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const mounted = useMounted()

  const apply = useCallback((channel: ImChannelView) => {
    setChannels((current) => {
      const index = current.findIndex((row) => row.channel.id === channel.id)
      if (index === -1) {
        return [...current, { channel, runtime: { state: 'disabled' } }]
      }
      // The read model never carries the runtime state; keep what we have.
      return current.map((row, position) =>
        position === index ? { ...row, channel } : row
      )
    })
  }, [])

  const refresh = useCallback(async () => {
    setStatus((prev) => (prev === 'ready' ? prev : 'loading'))
    try {
      const response = await listImChannels()
      if (!mounted.current) return
      setChannels(response.channels ?? [])
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
    async (request: CreateImChannelRequest): Promise<Outcome<ImChannelView>> => {
      const outcome = await mutate('create', () => createImChannel(request))
      if (outcome.ok && mounted.current) {
        apply(outcome.value)
        setStatus('ready')
      }
      return outcome
    },
    [apply, mutate]
  )

  const update = useCallback(
    async (channelId: string, request: UpdateImChannelRequest): Promise<Outcome<ImChannelView>> => {
      const outcome = await mutate(`update:${channelId}`, () => updateImChannel(channelId, request))
      if (outcome.ok && mounted.current) apply(outcome.value)
      return outcome
    },
    [apply, mutate]
  )

  const remove = useCallback(
    async (channelId: string): Promise<Outcome<void>> => {
      const outcome = await mutate(`delete:${channelId}`, () => deleteImChannel(channelId))
      if (outcome.ok && mounted.current) {
        setChannels((current) => current.filter((row) => row.channel.id !== channelId))
      }
      return outcome
    },
    [mutate]
  )

  const verify = useCallback(
    async (channelId: string): Promise<Outcome<string>> => {
      const outcome = await mutate(`verify:${channelId}`, async () => {
        const response = await verifyImChannel(channelId)
        return response.identity
      })
      return outcome
    },
    [mutate]
  )

  const loginStart = useCallback(
    async (): Promise<Outcome<{ sessionId: string; qrContent: string }>> => {
      return mutate('loginStart', async () => {
        const response = await startWeixinLogin()
        return { sessionId: response.session_id, qrContent: response.qr_content }
      })
    },
    [mutate]
  )

  const loginWait = useCallback(
    async (sessionId: string): Promise<Outcome<ImWeixinLoginWaitResponse>> => {
      const outcome = await mutate('loginWait', () => waitWeixinLogin(sessionId))
      if (outcome.ok && mounted.current && outcome.value.channel) {
        await refresh()
      }
      return outcome
    },
    [mutate, refresh]
  )

  useEffect(() => {
    void refresh()
  }, [refresh])

  return { channels, status, error, busy, refresh, create, update, remove, verify, loginStart, loginWait }
}
