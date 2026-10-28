/**
 * One live subscription per active thread.
 *
 * Sequence is: load the authoritative snapshot (`GET /v1/threads/{id}`), then
 * subscribe with `since_seq` = the snapshot watermark. The server guarantees the
 * backlog/live handoff is atomic (design §7.9), so no client-side dedupe is
 * needed and this hook must not add one.
 */
import { useEffect } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { ThreadId } from '../../../shared/contracts'
import { useToast } from '../../../shared/ui'
import { subscribeThreadEvents } from '../api/events-api'
import { getThread } from '../api/threads-api'
import { childCompletionStatus } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  if (error instanceof Error) return error.message
  return String(error)
}

export function useThreadStream(threadId: ThreadId | null): void {
  const seedItems = useCodeSessionStore((s) => s.seedItems)
  const resetItems = useCodeSessionStore((s) => s.resetItems)
  const ingestEvent = useCodeSessionStore((s) => s.ingestEvent)
  const setDetailStatus = useCodeSessionStore((s) => s.setDetailStatus)
  const setStreamStatus = useCodeSessionStore((s) => s.setStreamStatus)
  const { push: pushToast } = useToast()

  useEffect(() => {
    if (!threadId) {
      resetItems()
      setStreamStatus('closed')
      return
    }

    const controller = new AbortController()
    let cancelled = false
    const displayedCompletions = new Set<string>()
    setDetailStatus('loading')

    const run = async (): Promise<void> => {
      try {
        const detail = await getThread(threadId, controller.signal)
        if (cancelled) return
        const watermark = detail.summary.last_seq ?? 0
        seedItems(detail.items, watermark, detail.child_completions)

        await subscribeThreadEvents({
          threadId,
          sinceSeq: watermark,
          signal: controller.signal,
          onEvent: (event) => {
            if (cancelled) return
            ingestEvent(event)
            if (event.kind === 'child_completed' && !displayedCompletions.has(event.notification_id)) {
              displayedCompletions.add(event.notification_id)
              const detail = event.summary ?? event.error ?? '子代理未提供摘要。'
              const tone = event.status === 'completed' ? 'success' : 'error'
              pushToast(tone, `子代理「${event.label}」${childCompletionStatus(event.status)}`, detail)
            }
          },
          onStatus: (status) => {
            if (!cancelled) setStreamStatus(status)
          },
          onError: (error) => {
            if (!cancelled) setStreamStatus('reconnecting', describe(error))
          }
        })
      } catch (error) {
        if (cancelled || controller.signal.aborted) return
        setDetailStatus('error', describe(error))
      }
    }

    void run()

    return () => {
      cancelled = true
      controller.abort()
      setStreamStatus('closed')
    }
  }, [threadId, ingestEvent, resetItems, seedItems, setDetailStatus, setStreamStatus, pushToast])
}
