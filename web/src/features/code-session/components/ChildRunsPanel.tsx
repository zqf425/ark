import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { ChildRunListResponse } from '../../../shared/contracts'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { abortChildRun, listChildRuns, resumeChildRun, steerChildRun } from '../api/child-runs-api'
import { childCompletionStatusKey } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'
import { childRunMeta } from './child-run-meta'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function ChildRunsPanel({ threadId }: { threadId: string }): JSX.Element | null {
  const { t } = useI18n()
  const childRunRevision = useCodeSessionStore((state) => state.childRunRevision)
  const toast = useToast()
  const [data, setData] = useState<ChildRunListResponse | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const refresh = (): void => { void listChildRuns(threadId).then(setData).catch((error) => toast.push('error', t('canvas.childRuns.loadFailed'), describe(error))) }

  useEffect(() => {
    const controller = new AbortController()
    void listChildRuns(threadId, controller.signal).then(setData).catch(() => undefined)
    return () => controller.abort()
  }, [threadId, childRunRevision])

  if (!data || data.childRuns.length === 0) return null
  const run = async (key: string, action: () => Promise<unknown>): Promise<void> => {
    setBusy(key)
    try { await action(); refresh() } catch (error) { toast.push('error', t('canvas.childRuns.actionFailed'), describe(error)) } finally { setBusy(null) }
  }
  return <section className="ark-childruns"><header><strong>{t('canvas.childRuns.title')}</strong><span>{t('canvas.childRuns.active', { count: data.active })}</span></header>{data.childRuns.map((child) => {
    const active = child.status === 'queued' || child.status === 'running'
    const resumable = child.status === 'failed' || child.status === 'aborted'
    const meta = childRunMeta(child)
    return <div className="ark-childruns__row" key={child.id}><div><span className={`ark-chip ${active ? 'ark-chip--live' : child.status === 'completed' ? 'ark-chip--ok' : 'ark-chip--error'}`}>{t(childCompletionStatusKey(child.status))}</span><strong>{child.label}</strong>{meta.length > 0 ? <p className="ark-childruns__meta">{meta.map((part) => t(part.key, part.params)).join(' · ')}</p> : null}{child.summary ? <p>{child.summary}</p> : child.error ? <p>{child.error}</p> : null}</div><div className="ark-childruns__actions">{active ? <Button size="sm" variant="danger" disabled={busy !== null} onClick={() => void run(`abort:${child.id}`, () => abortChildRun(threadId, child.id))}>{busy === `abort:${child.id}` ? t('canvas.childRuns.aborting') : t('canvas.childRuns.abort')}</Button> : null}{resumable ? <Button size="sm" disabled={busy !== null} onClick={() => void run(`resume:${child.id}`, () => resumeChildRun(threadId, child.id))}>{t('canvas.childRuns.resume')}</Button> : null}<Button size="sm" disabled={!active || busy !== null} onClick={() => { const text = window.prompt(t('canvas.childRuns.steerPrompt')); if (text?.trim()) void run(`steer:${child.id}`, () => steerChildRun(threadId, child.id, text.trim())) }}>{t('canvas.childRuns.steer')}</Button></div></div>
  })}</section>
}
