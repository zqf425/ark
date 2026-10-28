import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { WorkspaceChangesResponse } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getChanges } from '../api/workbench-api'
import { changesSummary } from './changes-summary'
import { useSettings, useWorkspaceProbe } from '../../settings/hooks/use-settings'
import { workspaceStatusSummary } from '../../code-session/components/workspace-status-summary'
import { shouldProbeWorkspace } from '../../settings/hooks/workspace-probe-guard'
import './workspace-status.css'

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
  const { config } = useSettings()
  const workspace = useWorkspaceProbe()
  const workspacePath = config?.default_workspace?.trim() ?? ''
  const changeWatermark = data?.changes.map((change) => change.seq).join(',') ?? ''

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

  useEffect(() => {
    if (!shouldProbeWorkspace(threadId, workspacePath)) return
    void workspace.probe(workspacePath)
  }, [changeWatermark, workspace.probe, workspacePath])

  if (!threadId) return <EmptyState title="未选择会话" hint="选择会话后显示该会话成功写入的文件。" />
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">正在加载改动…</p>
  if (data.changes.length === 0) return <EmptyState title="暂无改动" hint="成功执行 write 或 edit 后会在这里出现。" />
  const workspaceStatus = workspace.status
  const workspaceSummary = workspaceStatus ? workspaceStatusSummary(workspaceStatus) : null
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>改动</h2>
        <span>{changesSummary(data.changes)}</span>
      </header>
      <p className="ark-panel__hint ark-panel__hint--flush">
        仅包含本会话成功执行的 write 或 edit；Git 差异以工作区状态为准。
      </p>
      {workspacePath ? (
        <div className={`ark-workspace-status${workspaceSummary ? ` ark-workspace-status--${workspaceSummary.tone}` : ''}`}>
          {workspaceSummary && workspaceStatus ? (
            <div className="ark-workspace-status__summary" title={`${workspaceStatus.path}：${workspaceSummary.detail}`}>
              <span className="ark-workspace-status__label">{workspaceSummary.label}</span>
              <span className="ark-workspace-status__detail">{workspaceSummary.detail}</span>
            </div>
          ) : (
            <span className="ark-workspace-status__detail">{workspace.error ?? '正在读取工作区状态…'}</span>
          )}
          <button className="ark-linkbtn" type="button" disabled={workspace.loading} onClick={() => void workspace.probe(workspacePath)}>
            {workspace.loading ? '刷新中…' : '刷新 Git 状态'}
          </button>
        </div>
      ) : (
        <p className="ark-panel__hint ark-panel__hint--flush">尚未设置默认工作空间，无法读取 Git 状态。</p>
      )}
      <div className="ark-panel__list">
        {data.changes.map((change) => (
          <button className="ark-panel__row" type="button" key={change.path} onClick={() => selectFile(change.path)}>
            <code>{change.path}</code>
            <span>{change.operation} · seq {change.seq}</span>
          </button>
        ))}
      </div>
    </section>
  )
}
