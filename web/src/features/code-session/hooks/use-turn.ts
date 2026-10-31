/**
 * Turn commands: start a turn, interrupt the active one.
 *
 * The hook never invents item rows: the user's message and the assistant answer
 * both arrive as `ThreadItem`s over the event stream (design §7.10.3). It only
 * reflects command state (`sending` / `interrupting`) and errors.
 */
import { useCallback } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { interruptTurn, startTurn, steerTurn } from '../api/turns-api'
import { useCodeSessionStore } from '../state/store'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export type TurnApi = {
  send: (text: string) => Promise<boolean>
  steer: (text: string) => Promise<boolean>
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
    async (text: string): Promise<boolean> => {
      const trimmed = text.trim()
      if (!threadId || !trimmed || sending) return false

      setSending(true)
      setCommandError(null)
      try {
        await startTurn(threadId, { text: trimmed })
        return true
      } catch (error) {
        setCommandError(describe(error))
        return false
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

  const steer = useCallback(async (text: string): Promise<boolean> => {
    const trimmed = text.trim()
    if (!threadId || !activeTurnId || !trimmed || sending) return false
    setSending(true)
    setCommandError(null)
    try {
      await steerTurn(threadId, activeTurnId, trimmed)
      return true
    } catch (error) {
      setCommandError(describe(error))
      return false
    } finally {
      setSending(false)
    }
  }, [activeTurnId, sending, setCommandError, setSending, threadId])

  return {
    send,
    steer,
    interrupt,
    canSend: Boolean(threadId) && streamStatus === 'open' && !sending,
    canInterrupt: Boolean(threadId) && Boolean(activeTurnId) && !interrupting
  }
}
