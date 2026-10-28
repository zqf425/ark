import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { TerminalEntriesResponse } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getTerminalEntries } from '../api/workbench-api'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  return error instanceof Error ? error.message : String(error)
}

export function TerminalPanel(): JSX.Element {
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const lastSeq = useCodeSessionStore((state) => state.lastSeq)
  const [data, setData] = useState<TerminalEntriesResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!threadId) return
    const controller = new AbortController()
    void getTerminalEntries(threadId, controller.signal).then((next) => {
      setData(next)
      setError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) setError(describe(reason))
    })
    return () => controller.abort()
  }, [threadId, lastSeq])

  if (!threadId) return <EmptyState title="未选择会话" hint="选择会话后显示该会话已完成的 bash 命令。" />
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">正在加载终端输出…</p>
  if (data.entries.length === 0) return <EmptyState title="无终端命令" hint="agent 执行 bash 后，命令和受限输出会显示在这里。" />
  return <section className="ark-panel"><header className="ark-panel__head"><h2>终端</h2><span>{data.entries.length} 条</span></header><div className="ark-terminal__list">{data.entries.map((entry) => <details className="ark-terminal" key={`${entry.call_id}:${entry.seq}`}><summary><span className={entry.ok ? 'ark-chip ark-chip--ok' : 'ark-chip ark-chip--error'}>{entry.ok ? '成功' : '失败'}</span><code>{entry.command}</code><span>{entry.duration_ms} ms</span></summary><pre className="ark-panel__code">{entry.output || '（无输出）'}</pre></details>)}</div></section>
}
