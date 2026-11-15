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
 * tool calls, reasoning and streaming text look identical in both places.
 */
import { useEffect, useRef, useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { getChildRunEvents, getChildRunThread } from '../api/child-runs-api'
import { foldChildEvent, seedChildTranscript, type ChildTranscript } from './child-transcript'
import { StreamRow } from './TurnBlock'

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
  return (
    <div className="ark-childruns__viewerBody">
      {transcript.itemOrder.length === 0 ? (
        <p className="ark-childruns__viewHint">{t('canvas.childRuns.viewerEmpty')}</p>
      ) : (
        transcript.itemOrder.map((itemId) => {
          const item = transcript.itemsById[itemId] as ThreadItem
          // Child transcripts are view-only context: no prose file chips, no
          // inspector navigation from another thread's rows.
          return (
            <StreamRow key={itemId} item={item} threadId={null} onFileChip={() => undefined} writtenPaths={EMPTY_PATHS} />
          )
        })
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
