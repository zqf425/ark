import { useEffect, useRef, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { ChildRunListResponse, ChildRunSummary } from '../../../shared/contracts'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { abortChildRun, listChildRuns, resumeChildRun, steerChildRun } from '../api/child-runs-api'
import { childCompletionStatusKey } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'
import { childRunMeta } from './child-run-meta'
import { ChildRunViewer } from './ChildRunViewer'

function describe(error: unknown): string {
  return describeError(error, translateNow)
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

  const active = child.status === 'queued' || child.status === 'running'
  const resumable = child.status === 'failed' || child.status === 'aborted'
  const meta = childRunMeta(child)

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
    <div className={`ark-childruns__row${expanded ? ' is-open' : ''}`}>
      <button type="button" className="ark-childruns__summary" onClick={() => setExpanded((v) => !v)} aria-expanded={expanded}>
        <span className={`ark-childruns__caret${expanded ? ' is-open' : ''}`} aria-hidden="true">▸</span>
        <span className={`ark-chip ${active ? 'ark-chip--live' : child.status === 'completed' ? 'ark-chip--ok' : 'ark-chip--error'}`}>
          {t(childCompletionStatusKey(child.status))}
        </span>
        <strong>{child.label}</strong>
        {meta.length > 0 ? <span className="ark-childruns__meta">{meta.map((part) => t(part.key, part.params)).join(' · ')}</span> : null}
      </button>
      {child.summary || child.error ? <p className="ark-childruns__outcome">{child.summary ?? child.error}</p> : null}
      {expanded ? (
        <div className="ark-childruns__viewer">
          <ChildRunViewer threadId={threadId} childId={child.id} active={active} />
          {active ? (
            <div className="ark-childruns__steer">
              <textarea
                className="ark-childruns__steerInput"
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
  const latestLoad = useRef<{ threadId: string; ticket: number }>({ threadId, ticket: 0 })

  // Switching threads drops the previous thread's rows in the same commit, and
  // the thread change in `latestLoad` invalidates every request still in flight
  // for it.
  if (rowsFor !== threadId) {
    setRowsFor(threadId)
    setData(null)
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
  return <section className="ark-childruns"><header><strong>{t('canvas.childRuns.title')}</strong><span>{t('canvas.childRuns.active', { count: data.active })}</span></header>{data.child_runs.map((child) => <ChildRunRow key={child.id} threadId={threadId} child={child} onChanged={refresh} />)}</section>
}
