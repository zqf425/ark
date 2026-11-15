import { useEffect, useRef, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { ChildRunListResponse, ChildRunSummary } from '../../../shared/contracts'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { abortChildRun, listChildRuns, resumeChildRun, steerChildRun } from '../api/child-runs-api'
import { childCompletionStatusKey } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'
import { childRunDuration, childRunMeta, formatChildRunDuration } from './child-run-meta'
import { ChildRunViewer } from './ChildRunViewer'
import { ChildRunStatusDot } from './ChildRunStatusDot'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

/** Settled runs collapse into one muted "finished" group after this many. */
const FINISHED_VISIBLE = 3

/**
 * Lifecycle state of one child run, resolved once per render: the status dot
 * class, the chip class and the busy/resumable flags all derive from it, so no
 * call site re-derives "is this one running?" from the raw status string.
 */
function statusOf(child: ChildRunSummary): {
  state: 'queued' | 'running' | 'completed' | 'failed' | 'aborted' | 'ended'
  active: boolean
  resumable: boolean
  settled: boolean
} {
  switch (child.status) {
    case 'queued':
      return { state: 'queued', active: true, resumable: false, settled: false }
    case 'running':
      return { state: 'running', active: true, resumable: false, settled: false }
    case 'completed':
      return { state: 'completed', active: false, resumable: false, settled: true }
    case 'failed':
      return { state: 'failed', active: false, resumable: true, settled: true }
    case 'aborted':
      return { state: 'aborted', active: false, resumable: true, settled: true }
    default:
      return { state: 'ended', active: false, resumable: false, settled: true }
  }
}

/**
 * One expanded row owns its steer draft and its transcript. Local component
 * state (not the canvas store): the draft dies with the row, a thread switch
 * unmounts it, and the parent stream must not re-render on a keystroke here.
 */
function ChildRunRow({
  threadId,
  child,
  onChanged
}: {
  threadId: string
  child: ChildRunSummary
  /** Re-read the list after a successful action (abort/resume/steer mutate it). */
  onChanged: () => void
}): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const [expanded, setExpanded] = useState(false)
  const [busy, setBusy] = useState<string | null>(null)
  const [steerDraft, setSteerDraft] = useState('')

  const status = statusOf(child)
  const active = status.active
  const resumable = status.resumable
  const meta = childRunMeta(child)
  const durationMs = childRunDuration(child)

  const run = async (key: string, action: () => Promise<unknown>): Promise<void> => {
    setBusy(key)
    try {
      await action()
      setSteerDraft('')
      onChanged()
    } catch (error) {
      toast.push('error', t('canvas.childRuns.actionFailed'), describe(error))
    } finally {
      setBusy(null)
    }
  }

  const sendSteer = (): void => {
    const text = steerDraft.trim()
    if (!text || busy) return
    void run(`steer:${child.id}`, () => steerChildRun(threadId, child.id, text))
  }

  return (
    <div className={`ark-childrow${expanded ? ' is-open' : ''}`}>
      <button
        type="button"
        className="ark-childrow__head"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <span className={`ark-childrow__chevron${expanded ? ' is-open' : ''}`} aria-hidden="true">
          {expanded ? '▾' : '▸'}
        </span>
        <ChildRunStatusDot state={status.state} />
        <span className="ark-childrow__label" title={child.label}>
          {child.label}
        </span>
        {durationMs !== null ? (
          <span className="ark-childrow__duration">{formatChildRunDuration(durationMs)}</span>
        ) : null}
        {meta.length > 0 ? (
          <span className="ark-childrow__meta">{meta.map((part) => t(part.key, part.params)).join(' · ')}</span>
        ) : null}
        {!active && !status.settled ? null : (
          <span className={`ark-chip ${active ? 'ark-chip--live' : status.state === 'failed' || status.state === 'aborted' ? 'ark-chip--error' : ''}`}>
            {t(childCompletionStatusKey(child.status))}
          </span>
        )}
      </button>
      {expanded ? (
        <div className="ark-childrow__viewer">
          <div className="ark-childrow__viewerHead">
            <span className="ark-childrow__viewerTitle">{t('canvas.childRuns.viewerTitle')}</span>
            {active ? <span className="ark-dot ark-dot--live" aria-hidden="true" /> : null}
          </div>
          <ChildRunViewer threadId={threadId} childId={child.id} active={active} />
          {active ? (
            <div className="ark-childrow__steer">
              <textarea
                className="ark-childrow__steerInput"
                rows={2}
                value={steerDraft}
                placeholder={t('canvas.childRuns.steerPlaceholder')}
                aria-label={t('canvas.childRuns.steerAria')}
                onChange={(event) => setSteerDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault()
                    sendSteer()
                  }
                }}
              />
              <div className="ark-childruns__actions">
                <Button size="sm" disabled={!steerDraft.trim() || busy !== null} onClick={sendSteer}>
                  {t('canvas.childRuns.steerSend')}
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  disabled={busy !== null}
                  onClick={() => void run(`abort:${child.id}`, () => abortChildRun(threadId, child.id))}
                >
                  {busy === `abort:${child.id}` ? t('canvas.childRuns.aborting') : t('canvas.childRuns.abort')}
                </Button>
              </div>
            </div>
          ) : null}
          {resumable ? (
            <div className="ark-childruns__actions">
              <Button size="sm" disabled={busy !== null} onClick={() => void run(`resume:${child.id}`, () => resumeChildRun(threadId, child.id))}>
                {t('canvas.childRuns.resume')}
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export function ChildRunsPanel({ threadId }: { threadId: string }): JSX.Element | null {
  const { t } = useI18n()
  const childRunRevision = useCodeSessionStore((state) => state.childRunRevision)
  const toast = useToast()
  const [data, setData] = useState<ChildRunListResponse | null>(null)
  /**
   * Which thread the rows and the newest list request belong to. Bookkeeping,
   * never render input: the panel stays mounted across thread switches while its
   * rows are per-thread, so "these rows are A's" has to be an explicit fact.
   */
  const [rowsFor, setRowsFor] = useState(threadId)
  const [showAllFinished, setShowAllFinished] = useState(false)
  const latestLoad = useRef<{ threadId: string; ticket: number }>({ threadId, ticket: 0 })

  // Switching threads drops the previous thread's rows in the same commit, and
  // the thread change in `latestLoad` invalidates every request still in flight
  // for it.
  if (rowsFor !== threadId) {
    setRowsFor(threadId)
    setData(null)
    setShowAllFinished(false)
    latestLoad.current = { threadId, ticket: latestLoad.current.ticket + 1 }
  }

  /**
   * One list request. A response paints only while it is still the newest
   * request for the thread on screen: the effect load and a post-action
   * `refresh()` can otherwise resolve out of order, and the previous thread's
   * rows must never reappear over the current one.
   */
  const load = (signal?: AbortSignal, onError?: (error: unknown) => void): void => {
    const ticket = latestLoad.current.ticket + 1
    latestLoad.current = { threadId, ticket }
    const isCurrent = (): boolean => !signal?.aborted && latestLoad.current.threadId === threadId && latestLoad.current.ticket === ticket
    void listChildRuns(threadId, signal).then((next) => {
      if (!isCurrent()) return
      setData(next)
    }).catch((error: unknown) => {
      if (!isCurrent()) return
      setData(null)
      onError?.(error)
    })
  }

  const refresh = (): void => {
    load(undefined, (error) => toast.push('error', t('canvas.childRuns.loadFailed'), describe(error)))
  }

  useEffect(() => {
    const controller = new AbortController()
    load(controller.signal)
    return () => controller.abort()
  }, [threadId, childRunRevision])

  if (!data || data.child_runs.length === 0) return null

  // Claude-style ordering: everything with a pulse sits on top, settled runs
  // sink below and overflow into one expandable count so old runs stop
  // crowding the live ones.
  const rows = [...data.child_runs].sort((a, b) => {
    const aSettled = statusOf(a).settled ? 1 : 0
    const bSettled = statusOf(b).settled ? 1 : 0
    if (aSettled !== bSettled) return aSettled - bSettled
    return a.created_at < b.created_at ? 1 : -1
  })
  const settledRows = rows.filter((child) => statusOf(child).settled)
  const hiddenFinished = showAllFinished ? 0 : Math.max(0, settledRows.length - FINISHED_VISIBLE)
  const visibleRows = showAllFinished ? rows : rows.slice(0, rows.length - hiddenFinished)

  return (
    <section className="ark-childruns">
      <header>
        <span className="ark-childruns__heading">
          <span className="ark-childruns__glyph" aria-hidden="true" />
          <strong>{t('canvas.childRuns.title')}</strong>
        </span>
        <span className="ark-childruns__count" data-active={data.active > 0 ? 'yes' : 'no'}>
          {data.active > 0 ? t('canvas.childRuns.active', { count: data.active }) : t('canvas.childRuns.idle', { count: data.child_runs.length })}
        </span>
      </header>
      <div className="ark-childruns__list">
        {visibleRows.map((child) => (
          <ChildRunRow key={child.id} threadId={threadId} child={child} onChanged={refresh} />
        ))}
        {hiddenFinished > 0 ? (
          <button
            type="button"
            className="ark-childruns__more"
            onClick={() => setShowAllFinished(true)}
          >
            {t('canvas.childRuns.showMore', { count: hiddenFinished })}
          </button>
        ) : null}
      </div>
    </section>
  )
}
