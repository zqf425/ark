import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { WorkspaceChangesResponse, WorkspaceDiff, WorkspaceView } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getChanges } from '../api/workbench-api'
import { changesSummary } from './changes-summary'
import { getWorkspaceDiff, getWorkspaceView, switchWorkspace } from '../../settings/api/workspace-api'
import { workspaceDiffSummary } from './workspace-diff-summary'
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
  const [workspaceView, setWorkspaceView] = useState<WorkspaceView | null>(null)
  const [workspaceDiff, setWorkspaceDiff] = useState<WorkspaceDiff | null>(null)
  const [workspaceError, setWorkspaceError] = useState<string | null>(null)
  const [workspaceDraft, setWorkspaceDraft] = useState('')
  const [switching, setSwitching] = useState(false)

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
    const controller = new AbortController()
    void Promise.all([getWorkspaceView(controller.signal), getWorkspaceDiff(controller.signal)]).then(([view, diff]) => {
      setWorkspaceView(view)
      setWorkspaceDiff(diff)
      setWorkspaceDraft((current) => current || view.active?.path || '')
      setWorkspaceError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) setWorkspaceError(describe(reason))
    })
    return () => controller.abort()
  }, [threadId, lastSeq])

  const refreshWorkspace = (): void => {
    void Promise.all([getWorkspaceView(), getWorkspaceDiff()]).then(([view, diff]) => {
      setWorkspaceView(view)
      setWorkspaceDiff(diff)
      setWorkspaceError(null)
    }).catch((reason) => setWorkspaceError(describe(reason)))
  }

  const changeWorkspace = async (): Promise<void> => {
    const path = workspaceDraft.trim()
    if (!path) return
    setSwitching(true)
    try {
      const view = await switchWorkspace({ path })
      const diff = await getWorkspaceDiff()
      setWorkspaceView(view)
      setWorkspaceDiff(diff)
      setWorkspaceError(null)
    } catch (reason) {
      setWorkspaceError(describe(reason))
    } finally {
      setSwitching(false)
    }
  }

  if (!threadId) return <EmptyState title="未选择会话" hint="选择会话后显示该会话成功写入的文件。" />
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">正在加载改动…</p>
  const gitSummary = workspaceDiff ? workspaceDiffSummary(workspaceDiff) : null
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>改动</h2>
        <span>{changesSummary(data.changes)}</span>
      </header>
      {data.changes.length === 0 ? <EmptyState title="暂无会话写入记录" hint="Git 差异仍会显示活动工作区中的现有改动。" /> : null}
      <p className="ark-panel__hint ark-panel__hint--flush">
        仅包含本会话成功执行的 write 或 edit；Git 差异以工作区状态为准。
      </p>
      <div className="ark-workspace-switch">
        <label htmlFor="changes-workspace">本次运行工作区</label>
        <div className="ark-workspace-switch__controls">
          <input
            id="changes-workspace"
            value={workspaceDraft}
            spellCheck={false}
            autoComplete="off"
            placeholder="输入已有工作区路径"
            onChange={(event) => setWorkspaceDraft(event.target.value)}
          />
          <button type="button" className="ark-linkbtn" disabled={switching || !workspaceDraft.trim()} onClick={() => void changeWorkspace()}>
            {switching ? '切换中…' : '切换'}
          </button>
          <button type="button" className="ark-linkbtn" disabled={switching} onClick={refreshWorkspace}>刷新差异</button>
        </div>
        {workspaceView?.active ? <p className="ark-panel__hint ark-panel__hint--flush">活动路径：{workspaceView.active.path}</p> : null}
        <p className="ark-panel__hint ark-panel__hint--flush">运行中存在父任务或子代理时不能切换；不会自动中止任务。</p>
        {workspaceError ? <p className="ark-panel__error">{workspaceError}</p> : null}
      </div>
      {gitSummary ? (
        <div className="ark-workspace-diff">
          <p className="ark-workspace-diff__summary">{gitSummary.staged} · {gitSummary.unstaged} · {gitSummary.untracked}</p>
          {gitSummary.truncated ? <p className="ark-panel__hint ark-panel__hint--flush">差异超出安全上限，当前仅显示可用部分。</p> : null}
          {([['已暂存', workspaceDiff?.staged ?? []], ['未暂存', workspaceDiff?.unstaged ?? []], ['未跟踪', workspaceDiff?.untracked ?? []]] as const)
            .filter(([, files]) => files.length > 0)
            .map(([label, files]) => (
              <section key={label} className="ark-workspace-diff__group">
                <h3>{label}</h3>
                {files.map((file) => <code key={`${file.status}:${file.path}`}>{file.status} {file.path}{file.truncated ? '（已截断）' : ''}{file.binary ? '（二进制）' : ''}</code>)}
              </section>
            ))}
        </div>
      ) : null}
      {data.changes.length > 0 ? (
        <div className="ark-panel__list">
          {data.changes.map((change) => (
            <button className="ark-panel__row" type="button" key={change.path} onClick={() => selectFile(change.path)}>
              <code>{change.path}</code>
              <span>{change.operation} · seq {change.seq}</span>
            </button>
          ))}
        </div>
      ) : null}
    </section>
  )
}
