import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { WorkspaceFilePreview } from '../../../shared/contracts'
import { EmptyState, highlightSource, Markdown } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getWorkspaceFilePreview } from '../api/workbench-api'
import { previewModeOf } from '../api/file-dispatch'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function FilePanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const path = useCodeSessionStore((state) => state.selectedFilePath)
  const workbenchRevision = useCodeSessionStore((state) => state.workbenchRevision)
  const [file, setFile] = useState<WorkspaceFilePreview | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!threadId || !path) {
      setFile(null)
      setError(null)
      return
    }
    const controller = new AbortController()
    void getWorkspaceFilePreview(path, threadId, controller.signal).then((next) => {
      setFile(next)
      setError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) {
        setFile(null)
        setError(describe(reason))
      }
    })
    return () => controller.abort()
  }, [threadId, path, workbenchRevision])

  if (!threadId || !path) {
    return <EmptyState title={t('workbench.noFileTitle')} hint={t('workbench.noFileHint')} />
  }
  if (error) {
    return (
      <EmptyState
        title={t('workbench.filePreviewFailedTitle')}
        hint={t('workbench.filePreviewFailedHint', { error })}
      />
    )
  }
  if (!file) return <p className="ark-panel__hint">{t('workbench.filePreviewLoading')}</p>
  const mode = previewModeOf(file.path, file.binary)
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>{file.path}</h2>
        {mode === 'markdown' ? <span>md</span> : null}
      </header>
      {mode === 'binary' ? (
        <EmptyState title={t('workbench.fileBinaryTitle')} hint={t('workbench.fileBinaryHint', { size: file.size_bytes })} />
      ) : mode === 'markdown' ? (
        <div className="ark-panel__markdown">
          <Markdown text={file.content} />
        </div>
      ) : (
        <pre className="ark-panel__code ark-codehl">
          <code className="hljs" dangerouslySetInnerHTML={{ __html: highlightSource(file.content, file.path) }} />
        </pre>
      )}
      {file.truncated ? <p className="ark-panel__hint">{t('workbench.fileTruncated')}</p> : null}
    </section>
  )
}
