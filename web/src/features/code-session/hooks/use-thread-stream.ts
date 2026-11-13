/**
 * One live subscription per active thread.
 *
 * Sequence is: load the authoritative snapshot (`GET /v1/threads/{id}`), then
 * subscribe with `since_seq` = the snapshot watermark. The server guarantees the
 * backlog/live handoff is atomic (design §7.9), so no client-side dedupe is
 * needed and this hook must not add one.
 */
import { useEffect } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { ThreadId } from '../../../shared/contracts'
import { useToast } from '../../../shared/ui'
import { subscribeThreadEvents } from '../api/events-api'
import { getThread, getThreadUsage } from '../api/threads-api'
import { childCompletionStatusKey } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function useThreadStream(threadId: ThreadId | null): void {
  const seedItems = useCodeSessionStore((s) => s.seedItems)
  const resetItems = useCodeSessionStore((s) => s.resetItems)
  const ingestEvent = useCodeSessionStore((s) => s.ingestEvent)
  const setDetailStatus = useCodeSessionStore((s) => s.setDetailStatus)
  const setStreamStatus = useCodeSessionStore((s) => s.setStreamStatus)
  const noteThreadUsage = useCodeSessionStore((s) => s.noteThreadUsage)
  const noteThreadSeq = useCodeSessionStore((s) => s.noteThreadSeq)
  const noteThreadActivity = useCodeSessionStore((s) => s.noteThreadActivity)
  // A rewind physically truncates history; bumping this token re-runs the whole
  // load + subscribe sequence from the fresh detail watermark.
  const reloadToken = useCodeSessionStore((s) => s.reloadToken)
  const requestStreamReload = useCodeSessionStore((s) => s.requestStreamReload)
  const { push: pushToast } = useToast()

  useEffect(() => {
    if (!threadId) {
      resetItems()
      setStreamStatus('closed')
      return
    }

    const controller = new AbortController()
    let cancelled = false
    // One rebuild per subscription: the token bump tears this subscription down
    // anyway, so a second notification in the same backlog must not queue one.
    let rebuildScheduled = false
    const displayedCompletions = new Set<string>()
    // A load belongs to this thread only. Dropping the previous projection before
    // the first await keeps A's items, turn states and approval/user-input gates
    // off screen while B is loading, so nothing from A can still be answered in
    // B's canvas. The watermark below still comes from B's own detail.
    resetItems()
    setDetailStatus('loading')

    const run = async (): Promise<void> => {
      try {
        const detail = await getThread(threadId, controller.signal)
        if (cancelled) return
        const watermark = detail.summary.last_seq ?? 0
        seedItems(detail.items, watermark, detail.child_completions, detail.runtime_snapshot)
        // The detail watermark is authoritative; pull the rail row up to it in
        // case the list was loaded before the newest events landed.
        noteThreadSeq(threadId, watermark)
        // Same for the busy marker and the activity key: the detail snapshot
        // is fresher than any list page the rail may have been seeded from.
        if (detail.summary.last_event_at) noteThreadActivity(threadId, detail.summary.last_event_at)

        // Usage history is event-folded server-side, not part of the item
        // stream; seed it once per load. Best effort: an unavailable counter
        // must not block the canvas, and the reducer only fills an empty
        // channel, so a live `usage` event can never be double counted.
        void getThreadUsage(threadId, controller.signal)
          .then((usage) => {
            if (!cancelled) noteThreadUsage(usage)
          })
          .catch(() => {
            // Seeding is best effort, but "footer shows no usage" must stay
            // diagnosable: surface the failure in development like the
            // reducer's unhandled-event warn.
            if (import.meta.env.DEV) console.warn('[ark] thread usage seed failed')
          })

        await subscribeThreadEvents({
          threadId,
          sinceSeq: watermark,
          signal: controller.signal,
          onEvent: (event) => {
            if (cancelled) return
            ingestEvent(event)
            // Delta events arrive once per streamed token; forwarding every
            // seq would re-render the whole thread rail for each character.
            // Milestones carry the newest seq, so the rail stays current
            // without a render per token (`noteThreadSeq` is monotonic).
            if (
              event.thread_id === threadId &&
              event.kind !== 'assistant_text_delta' &&
              event.kind !== 'assistant_reasoning_delta'
            ) {
              noteThreadSeq(threadId, event.seq)
              noteThreadActivity(threadId, event.timestamp)
            }
            // A delta is the strongest "the model is replying" signal: it
            // moves the row to the rail top right away. Throttled by the same
            // monotonic guard as the seq note (older timestamps are dropped),
            // so a burst of tokens costs one sort per event at worst.
            else if (event.thread_id === threadId) {
              noteThreadActivity(threadId, event.timestamp)
            }
            // `thread_rewound` says the durable log was truncated and this seq
            // is a fresh, never-reused notification (docs/modules/ark-store.md).
            // Folding it is not enough: rows from the discarded suffix are gone
            // on the server, so re-seed from a new detail watermark instead of
            // trusting the local projection.
            if (event.kind === 'thread_rewound') {
              if (!rebuildScheduled) {
                rebuildScheduled = true
                requestStreamReload()
              }
              return
            }
            if (event.kind === 'child_completed' && !displayedCompletions.has(event.notification_id)) {
              displayedCompletions.add(event.notification_id)
              const detail = event.summary ?? event.error ?? translateNow('canvas.child.noSummary')
              const tone = event.status === 'completed' ? 'success' : 'error'
              pushToast(
                tone,
                translateNow('canvas.childRuns.completedToast', {
                  label: event.label,
                  status: translateNow(childCompletionStatusKey(event.status))
                }),
                detail
              )
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
        // A failed load leaves nothing to show: keeping the previous thread's
        // rows would present stale history under the new thread's error.
        resetItems()
        setDetailStatus('error', describe(error))
      }
    }

    void run()

    return () => {
      cancelled = true
      controller.abort()
      setStreamStatus('closed')
    }
  }, [threadId, reloadToken, ingestEvent, noteThreadActivity, noteThreadSeq, noteThreadUsage, requestStreamReload, resetItems, seedItems, setDetailStatus, setStreamStatus, pushToast])
}
