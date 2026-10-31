import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { WorkspaceChangesResponse, WorkspaceDiff, WorkspaceView } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getChanges } from '../api/workbench-api'
import { changesSummary } from './changes-summary'
import { getWorkspaceDiff, getWorkspaceView, switchWorkspace } from '../../settings/api/workspace-api'
import { workspaceDiffSummary } from './workspace-diff-summary'
import './workspace-status.css'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function ChangesPanel(): JSX.Element {
  const { t } = useI18n()
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

  if (!threadId) {
    return <EmptyState title={t('workbench.noThreadTitle')} hint={t('workbench.noThreadChangesHint')} />
  }
  if (error) return <p className="ark-panel__error">{error}</p>
  if (!data) return <p className="ark-panel__hint">{t('workbench.changes.loading')}</p>
  const gitSummary = workspaceDiff ? workspaceDiffSummary(workspaceDiff) : null
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>{t('workbench.changes.title')}</h2>
        <span>{changesSummary(data.changes).map((part) => t(part.key, part.params)).join(' · ')}</span>
      </header>
      {data.changes.length === 0 ? (
        <EmptyState title={t('workbench.changes.emptyTitle')} hint={t('workbench.changes.emptyHint')} />
      ) : null}
      <p className="ark-panel__hint ark-panel__hint--flush">{t('workbench.changes.scopeHint')}</p>
      <div className="ark-workspace-switch">
        <label htmlFor="changes-workspace">{t('workbench.changes.workspaceLabel')}</label>
        <div className="ark-workspace-switch__controls">
          <input
            id="changes-workspace"
            value={workspaceDraft}
            spellCheck={false}
            autoComplete="off"
            placeholder={t('workbench.changes.workspacePlaceholder')}
            onChange={(event) => setWorkspaceDraft(event.target.value)}
          />
          <button type="button" className="ark-linkbtn" disabled={switching || !workspaceDraft.trim()} onClick={() => void changeWorkspace()}>
            {switching ? t('workbench.changes.switching') : t('workbench.changes.switch')}
          </button>
          <button type="button" className="ark-linkbtn" disabled={switching} onClick={refreshWorkspace}>
            {t('workbench.changes.refreshDiff')}
          </button>
        </div>
        {workspaceView?.active ? (
          <p className="ark-panel__hint ark-panel__hint--flush">
            {t('workbench.changes.activePath', { path: workspaceView.active.path })}
          </p>
        ) : null}
        <p className="ark-panel__hint ark-panel__hint--flush">{t('workbench.changes.switchBlocked')}</p>
        {workspaceError ? <p className="ark-panel__error">{workspaceError}</p> : null}
      </div>
      {gitSummary ? (
        <div className="ark-workspace-diff">
          <p className="ark-workspace-diff__summary">
            {t(gitSummary.staged.key, gitSummary.staged.params)} · {t(gitSummary.unstaged.key, gitSummary.unstaged.params)} ·{' '}
            {t(gitSummary.untracked.key, gitSummary.untracked.params)}
          </p>
          {gitSummary.truncated ? (
            <p className="ark-panel__hint ark-panel__hint--flush">{t('workbench.changes.truncated')}</p>
          ) : null}
          {([
            [t('workbench.changes.staged'), workspaceDiff?.staged ?? []],
            [t('workbench.changes.unstaged'), workspaceDiff?.unstaged ?? []],
            [t('workbench.changes.untracked'), workspaceDiff?.untracked ?? []]
          ] as const)
            .filter(([, files]) => files.length > 0)
            .map(([label, files]) => (
              <section key={label} className="ark-workspace-diff__group">
                <h3>{label}</h3>
                {files.map((file) => (
                  <code key={`${file.status}:${file.path}`}>
                    {file.status} {file.path}
                    {file.truncated ? t('workbench.changes.truncatedFile') : ''}
                    {file.binary ? t('workbench.changes.binaryFile') : ''}
                  </code>
                ))}
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
