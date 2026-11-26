/**
 * Turn commands: start a turn, interrupt the active one.
 *
 * The hook never invents item rows: the user's message and the assistant answer
 * both arrive as `ThreadItem`s over the event stream (design §7.10.3). It only
 * reflects command state (`sending` / `interrupting`) and errors.
 */
import { useCallback } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { ApiError } from '../../../shared/api/client'
import { translateNow } from '../../../shared/i18n/current'
import { interruptTurn, startTurn, steerTurn } from '../api/turns-api'
import { useCodeSessionStore } from '../state/store'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export type TurnSendResult = 'accepted' | 'retry' | 'permanent'

function failureResult(error: unknown): TurnSendResult {
  // A transport failure can happen after arkd accepted the turn. Resending
  // would duplicate user work without an explicit idempotency contract.
  if (!(error instanceof ApiError)) return 'permanent'
  return error.status >= 500 || error.status === 408 || error.status === 425 || error.status === 429
    ? 'retry'
    : 'permanent'
}

export type TurnApi = {
  send: (text: string, attachmentIds?: string[]) => Promise<TurnSendResult>
  steer: (text: string, attachmentIds?: string[]) => Promise<TurnSendResult>
  interrupt: () => Promise<void>
  canSend: boolean
  canInterrupt: boolean
}

export function useTurn(threadId: string | null): TurnApi {
  const sending = useCodeSessionStore((s) => s.sending)
  const interrupting = useCodeSessionStore((s) => s.interrupting)
  const setSending = useCodeSessionStore((s) => s.setSending)
  const setInterrupting = useCodeSessionStore((s) => s.setInterrupting)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)

  const send = useCallback(
    async (text: string, attachmentIds: string[] = []): Promise<TurnSendResult> => {
      const trimmed = text.trim()
      if (!threadId || !trimmed || sending) return 'permanent'

      setSending(true)
      setCommandError(null)
      try {
        await startTurn(threadId, { text: trimmed, attachment_ids: attachmentIds })
        return 'accepted'
      } catch (error) {
        setCommandError(describe(error))
        return failureResult(error)
      } finally {
        setSending(false)
      }
    },
    [sending, setCommandError, setSending, threadId]
  )

  const interrupt = useCallback(async (): Promise<void> => {
    if (!threadId || !activeTurnId || interrupting) return
    setInterrupting(true)
    setCommandError(null)
    try {
      await interruptTurn(threadId, activeTurnId)
    } catch (error) {
      setCommandError(describe(error))
    } finally {
      setInterrupting(false)
    }
  }, [activeTurnId, interrupting, setCommandError, setInterrupting, threadId])

  const steer = useCallback(
    async (text: string, attachmentIds: string[] = []): Promise<TurnSendResult> => {
      const trimmed = text.trim()
      if (!threadId || !activeTurnId || !trimmed || sending) return 'permanent'
      setSending(true)
      setCommandError(null)
      try {
        await steerTurn(threadId, activeTurnId, { text: trimmed, attachment_ids: attachmentIds })
        return 'accepted'
      } catch (error) {
        setCommandError(describe(error))
        return failureResult(error)
      } finally {
        setSending(false)
      }
    },
    [activeTurnId, sending, setCommandError, setSending, threadId]
  )

  return {
    send,
    steer,
    interrupt,
    canSend: Boolean(threadId) && streamStatus === 'open' && !sending,
    canInterrupt: Boolean(threadId) && Boolean(activeTurnId) && !interrupting
  }
}
