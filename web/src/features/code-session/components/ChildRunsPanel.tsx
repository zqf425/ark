import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { ChildRunListResponse } from '../../../shared/contracts'
import { Button, useToast } from '../../../shared/ui'
import { abortChildRun, listChildRuns, resumeChildRun, steerChildRun } from '../api/child-runs-api'
import { childCompletionStatus } from '../child-completion-status'
import { useCodeSessionStore } from '../state/store'
import { childRunMeta } from './child-run-meta'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  return error instanceof Error ? error.message : String(error)
}

export function ChildRunsPanel({ threadId }: { threadId: string }): JSX.Element | null {
  const childRunRevision = useCodeSessionStore((state) => state.childRunRevision)
  const toast = useToast()
  const [data, setData] = useState<ChildRunListResponse | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const refresh = (): void => { void listChildRuns(threadId).then(setData).catch((error) => toast.push('error', '读取子代理失败', describe(error))) }

  useEffect(() => {
    const controller = new AbortController()
    void listChildRuns(threadId, controller.signal).then(setData).catch(() => undefined)
    return () => controller.abort()
  }, [threadId, childRunRevision])

  if (!data || data.childRuns.length === 0) return null
  const run = async (key: string, action: () => Promise<unknown>): Promise<void> => {
    setBusy(key)
    try { await action(); refresh() } catch (error) { toast.push('error', '子代理操作失败', describe(error)) } finally { setBusy(null) }
  }
  return <section className="ark-childruns"><header><strong>子代理</strong><span>{data.active} 活跃</span></header>{data.childRuns.map((child) => {
    const active = child.status === 'queued' || child.status === 'running'
    const resumable = child.status === 'failed' || child.status === 'aborted'
    const meta = childRunMeta(child)
    return <div className="ark-childruns__row" key={child.id}><div><span className={`ark-chip ${active ? 'ark-chip--live' : child.status === 'completed' ? 'ark-chip--ok' : 'ark-chip--error'}`}>{childCompletionStatus(child.status)}</span><strong>{child.label}</strong>{meta.length > 0 ? <p className="ark-childruns__meta">{meta.join(' · ')}</p> : null}{child.summary ? <p>{child.summary}</p> : child.error ? <p>{child.error}</p> : null}</div><div className="ark-childruns__actions">{active ? <Button size="sm" variant="danger" disabled={busy !== null} onClick={() => void run(`abort:${child.id}`, () => abortChildRun(threadId, child.id))}>{busy === `abort:${child.id}` ? '终止中…' : '终止'}</Button> : null}{resumable ? <Button size="sm" disabled={busy !== null} onClick={() => void run(`resume:${child.id}`, () => resumeChildRun(threadId, child.id))}>恢复</Button> : null}<Button size="sm" disabled={!active || busy !== null} onClick={() => { const text = window.prompt('补充给子代理的指令'); if (text?.trim()) void run(`steer:${child.id}`, () => steerChildRun(threadId, child.id, text.trim())) }}>补充指令</Button></div></div>
  })}</section>
}
