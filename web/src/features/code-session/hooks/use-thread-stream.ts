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
import type { RuntimeEvent, ThreadId } from '../../../shared/contracts'
import { useToast } from '../../../shared/ui'
import { subscribeThreadEvents } from '../api/events-api'
import { getThread, getThreadUsage } from '../api/threads-api'
import { listLlmRounds } from '../api/llm-rounds-api'
import { childCompletionStatusKey } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'
import { notifyTurnEnded } from './turn-end-toast'
function describe(error: unknown): string {
  return describeError(error, translateNow)
}

/**
 * Event kinds that advance the rail's activity key (and therefore re-sort the
 * thread list): the user posting a message (`turn_started`, `turn_steered`)
 * and the turn's terminal events.
 *
 * Every other durable kind — deltas, item upserts, tool calls, gates, usage —
 * only moves the watermark. The rail is seeded from the list endpoint, whose
 * rows reflect each thread's own last boundary event, so letting in-flight
 * events advance the key would keep the *subscribed* (i.e. clicked) thread
 * pinned to the top while several sessions run in parallel.
 */
const RAIL_ACTIVITY_EVENT_KINDS: ReadonlySet<RuntimeEvent['kind']> = new Set([
  'turn_started',
  'turn_steered',
  'turn_completed',
  'turn_failed',
  'turn_aborted'
])

export function useThreadStream(threadId: ThreadId | null): void {
  const seedItems = useCodeSessionStore((s) => s.seedItems)
  const setThreadRuntime = useCodeSessionStore((s) => s.setThreadRuntime)
  const resetItems = useCodeSessionStore((s) => s.resetItems)
  const ingestEvent = useCodeSessionStore((s) => s.ingestEvent)
  const setDetailStatus = useCodeSessionStore((s) => s.setDetailStatus)
  const setStreamStatus = useCodeSessionStore((s) => s.setStreamStatus)
  const noteThreadUsage = useCodeSessionStore((s) => s.noteThreadUsage)
  const noteThreadRounds = useCodeSessionStore((s) => s.noteThreadRounds)
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
        seedItems(detail.items, watermark, detail.runtime_snapshot)
        // The composer pills read their labels from here: this load already
        // carries the stored overrides, so they need no second detail fetch and
        // a switch back to a thread shows its own model immediately.
        setThreadRuntime(threadId, detail.runtime)
        // The detail watermark is authoritative; pull the rail row up to it in
        // case the list was loaded before the newest events landed.
        noteThreadSeq(threadId, watermark)
        // Deliberately NOT folding `detail.summary.last_event_at` into the
        // activity key: this load is triggered by selecting the thread, and a
        // running thread's detail key is fresher than the rail row (it moves
        // with every in-flight event server-side). Syncing it here would lift
        // whichever thread was merely clicked above the others. The row's key
        // is advanced only by the turn-boundary events below and by the
        // authoritative list refresh.
        // Same for the busy marker: the detail snapshot is fresher than any
        // list page the rail may have been seeded from, but the busy flag is
        // projected from `runtime_snapshot.active_turn` via the live turn
        // reducer, not from the activity key.

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

        // Per-round model timings come from the bounded diagnostics surface
        // and are authoritative over live-estimated rounds; same best-effort
        // seeding contract as the usage aggregate above.
        void listLlmRounds(threadId, controller.signal)
          .then((rounds) => {
            if (!cancelled) noteThreadRounds(rounds)
          })
          .catch(() => {
            if (import.meta.env.DEV) console.warn('[ark] thread rounds seed failed')
          })

        await subscribeThreadEvents({
          threadId,
          sinceSeq: watermark,
          signal: controller.signal,
          onEvent: (event) => {
            if (cancelled) return
            ingestEvent(event)
            // Event classes (mirrors the server-side projector:
            // transport/synthetic frames mutate no projection field):
            // - transport frames (heartbeat, sse_idle_timeout,
            //   transient_retry) carry no thread activity at all: a
            //   subscribe's opening heartbeat is stamped `Utc::now()` by the
            //   server, so counting it re-ordered the rail to put the thread
            //   that was merely *clicked* on top.
            if (
              event.kind === 'heartbeat' ||
              event.kind === 'sse_idle_timeout' ||
              event.kind === 'transient_retry'
            ) {
              return
            }
            if (event.thread_id === threadId) {
              // Non-delta kinds keep the rail's watermark current (display
              // and monotonic guard only — no reordering).
              if (
                event.kind !== 'assistant_text_delta' &&
                event.kind !== 'assistant_reasoning_delta'
              ) {
                noteThreadSeq(threadId, event.seq)
              }
              // The activity key — and with it the rail order — advances only
              // on turn boundaries: the user posted a message or the turn
              // ended. Deltas, tools and gates streaming in between must not
              // keep lifting the subscribed thread over its peers.
              if (RAIL_ACTIVITY_EVENT_KINDS.has(event.kind)) {
                noteThreadActivity(threadId, event.timestamp)
              }
              // Instant terminal-state toast. The durable timestamp is the
              // shared dedupe key with the list poll (see `turn-end-toast`):
              // whichever surface sees the end first toasts, the other stays
              // silent, and a later replay of the same terminal frame never
              // toasts again.
              if (
                event.kind === 'turn_completed' ||
                event.kind === 'turn_failed' ||
                event.kind === 'turn_aborted'
              ) {
                const outcome =
                  event.kind === 'turn_completed' ? 'completed'
                  : event.kind === 'turn_failed' ? 'failed'
                  : 'aborted'
                notifyTurnEnded({ threadId, outcome, endTime: event.timestamp })
              }
            }
            // A background rename (manual PATCH or the auto title generator)
            // lands on the rail immediately; the list endpoint stays
            // authoritative on the next refresh.
            if (event.thread_id === threadId && event.kind === 'thread_updated') {
              useCodeSessionStore.getState().renameThreadRow(threadId, event.title)
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
  }, [threadId, reloadToken, ingestEvent, noteThreadActivity, noteThreadRounds, noteThreadSeq, noteThreadUsage, requestStreamReload, resetItems, seedItems, setDetailStatus, setStreamStatus, pushToast])
}
