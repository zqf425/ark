import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { WorkspaceChangesResponse } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getChanges } from '../api/workbench-api'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  return error instanceof Error ? error.message : String(error)
}

export function ChangesPanel(): JSX.Element {
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const lastSeq = useCodeSessionStore((state) => state.lastSeq)
  const selectFile = useCodeSessionStore((state) => state.selectFile)
  const [data, setData] = useState<WorkspaceChangesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!threadId) return
    const controller = new AbortController()
    void getChanges(threadId, controller.signal).then((next) => {
      setData(next)
      setError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) setError(describe(reason))
    })
    return () => controller.abort()
  }, [threadId, lastSeq])

  if (!threadId) return <EmptyState title="未选择会话" hint="选择会话后显示该会话成功写入的文件。" />
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">正在加载改动…</p>
  if (data.changes.length === 0) return <EmptyState title="暂无改动" hint="成功执行 write 或 edit 后会在这里出现。" />
  return <section className="ark-panel"><header className="ark-panel__head"><h2>改动</h2><span>{data.changes.length} 个文件</span></header><div className="ark-panel__list">{data.changes.map((change) => <button className="ark-panel__row" type="button" key={change.path} onClick={() => selectFile(change.path)}><code>{change.path}</code><span>{change.operation} · seq {change.seq}</span></button>)}</div></section>
}
