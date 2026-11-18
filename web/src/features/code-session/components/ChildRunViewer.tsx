/**
 * Execution viewer for one expanded child run.
 *
 * Load path: seed from `GET /v1/child-runs/{id}/thread`, then poll
 * `GET /v1/child-runs/{id}/events` with `since_seq` until the run settles.
 * Polling (instead of an SSE subscription) matches the design note for these
 * routes: the panel is a secondary surface and the child stream has no
 * keep-alive semantics of its own, so a bounded poll keeps one HTTP connection
 * per *expanded* row instead of one per visible child.
 *
 * Rows render through the same `StreamRow` components as the parent canvas, so
 * tool calls, reasoning and streaming text look identical in both places — and
 * they fold the same way too: only the newest `RECENT_ACTIVITY_ROW_LIMIT`
 * activity rows stay visible while everything earlier collapses behind the
 * same summary row the parent process card uses (`splitProcessTrace`). The
 * child transcript is one flat trace (no turn grouping), so the whole
 * transcript is the process trace; errors and notices still render at their
 * event position and never fold. A fresh poll extends the tail, so the newest
 * activity always stays on screen without re-rendering the expanded prefix.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { getChildRunEvents, getChildRunThread } from '../api/child-runs-api'
import { foldChildEvent, seedChildTranscript, type ChildTranscript } from './child-transcript'
import {
  isFoldedProcessRow,
  RECENT_ACTIVITY_ROW_LIMIT,
  splitProcessTrace
} from './turn-sections'
import { FoldedProcessRow, StreamRow } from './TurnBlock'

/** Shared no-op whitelist: child transcripts render without prose chips. */
const EMPTY_PATHS: readonly string[] = []

/** Poll cadence while the child is still active. */
export const ACTIVE_POLL_INTERVAL_MS = 1_500

export function ChildRunViewer({
  threadId,
  childId,
  active
}: {
  threadId: string
  childId: string
  /** A settled run keeps one final poll after mount, then stops. */
  active: boolean
}): JSX.Element {
  const { t } = useI18n()
  const [transcript, setTranscript] = useState<ChildTranscript | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Whether the user lifted the earlier-steps fold. Local to the expanded row
  // like the transcript itself: a remount (collapse/expand of the run row)
  // falls back to the folded default, matching the parent canvas where a
  // settled turn reopens on "newest steps only".
  const [traceExpanded, setTraceExpanded] = useState(false)
  // Mount identity of this viewer: unmount/abort the in-flight request chain,
  // and a remount (expand/collapse) starts from the authoritative snapshot.
  const generation = useRef(0)

  useEffect(() => {
    const controller = new AbortController()
    const ticket = generation.current + 1
    generation.current = ticket
    const alive = (): boolean => !controller.signal.aborted && generation.current === ticket

    const poll = async (lastSeq: number, intervalMs: number): Promise<void> => {
      if (controller.signal.aborted) return
      try {
        const next = await getChildRunEvents(threadId, childId, lastSeq, controller.signal)
        if (!alive()) return
        setTranscript((prev) => {
          if (!prev) return prev
          let folded = prev
          for (const event of next.events) folded = foldChildEvent(folded, event)
          // `highest_seq` is the store watermark: a gap-free poll must not fall
          // behind it even when every frame was display-irrelevant.
          if (next.highest_seq > folded.lastSeq) folded = { ...folded, lastSeq: next.highest_seq }
          return folded
        })
        setError(null)
        if (active) window.setTimeout(() => void poll(next.highest_seq, intervalMs), intervalMs)
      } catch (requestError) {
        if (!alive() || controller.signal.aborted) return
        setError(describeError(requestError, translateNow))
        if (active) window.setTimeout(() => void poll(lastSeq, intervalMs), intervalMs)
      }
    }

    const run = async (): Promise<void> => {
      setTranscript(null)
      setError(null)
      try {
        const detail = await getChildRunThread(threadId, childId, controller.signal)
        if (!alive()) return
        const seeded = seedChildTranscript(detail.items, detail.summary.last_seq ?? 0)
        setTranscript(seeded)
        // `detail.summary.last_seq` seeds from items, while the event store may
        // hold lifecycle events beyond the last item; resume from the highest
        // watermark so no gap is silently skipped.
        await poll(seeded.lastSeq, ACTIVE_POLL_INTERVAL_MS)
      } catch (loadError) {
        if (!alive()) return
        setError(describeError(loadError, translateNow))
      }
    }

    void run()
    return () => {
      controller.abort()
    }
  }, [threadId, childId, active])

  // Same default view as the parent process card: only the newest activity
  // rows stay visible, earlier ones fold behind one summary row. The
  // transcript is one flat trace (no turn grouping), so the fold is computed
  // over all of it; recomputing per poll is fine, the poll cadence (1.5s) is
  // far above the render cost of slicing a list of row ids.
  const trace = useMemo(
    () =>
      transcript
        ? splitProcessTrace(
            transcript.itemOrder.map((id) => transcript.itemsById[id] as ThreadItem),
            RECENT_ACTIVITY_ROW_LIMIT
          )
        : null,
    [transcript]
  )

  if (error && !transcript) {
    return <p className="ark-childruns__viewError">{error}</p>
  }
  if (!transcript) {
    return (
      <div className="ark-childruns__loading" role="status">
        <span className="ark-childdot is-running" aria-hidden="true" />
        <span>{t('canvas.childRuns.viewerLoading')}</span>
      </div>
    )
  }
  // Whether the fold actually holds: a user toggle lifts it, exactly like the
  // parent card's `foldActive`. The earlier slice then renders only what the
  // fold must not hide (errors and notices); everything else in it is
  // represented by the summary row.
  const foldActive = trace !== null && !traceExpanded && trace.foldedActivityRows > 0
  const headRows =
    trace === null
      ? []
      : foldActive
        ? trace.prefix.filter((item) => !isFoldedProcessRow(item))
        : trace.prefix

  return (
    <div className="ark-childruns__viewerBody">
      {transcript.itemOrder.length === 0 ? (
        <p className="ark-childruns__viewHint">{t('canvas.childRuns.viewerEmpty')}</p>
      ) : (
        <>
          {headRows.map((item) => (
            <StreamRow
              key={item.item_id}
              item={item}
              threadId={null}
              onFileChip={() => undefined}
              writtenPaths={EMPTY_PATHS}
            />
          ))}
          {trace!.foldedActivityRows > 0 ? (
            <FoldedProcessRow
              count={trace!.foldedActivityRows}
              expanded={!foldActive}
              onToggle={() => setTraceExpanded((v) => !v)}
            />
          ) : null}
          {trace!.tail.map((item) => (
            <StreamRow
              key={item.item_id}
              item={item}
              threadId={null}
              onFileChip={() => undefined}
              writtenPaths={EMPTY_PATHS}
            />
          ))}
        </>
      )}
      {error ? <p className="ark-childruns__viewError">{t('canvas.childRuns.viewerStale', { error })}</p> : null}
      {active ? (
        <p className="ark-childruns__live">
          <span className="ark-childdot is-running" aria-hidden="true" />
          {t('canvas.childRuns.viewerLive')}
        </p>
      ) : null}
    </div>
  )
}
