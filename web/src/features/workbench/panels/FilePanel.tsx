import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { FilePreview } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getFilePreview } from '../api/workbench-api'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

export function FilePanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const path = useCodeSessionStore((state) => state.selectedFilePath)
  const lastSeq = useCodeSessionStore((state) => state.lastSeq)
  const [file, setFile] = useState<FilePreview | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!threadId || !path) {
      setFile(null)
      setError(null)
      return
    }
    const controller = new AbortController()
    void getFilePreview(threadId, path, controller.signal).then((next) => {
      setFile(next)
      setError(null)
    }).catch((reason) => {
      if (!controller.signal.aborted) {
        setFile(null)
        setError(describe(reason))
      }
    })
    return () => controller.abort()
  }, [threadId, path, lastSeq])

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
  return <section className="ark-panel"><header className="ark-panel__head"><h2>{file.path}</h2><span>seq {file.source_seq}</span></header><pre className="ark-panel__code">{file.content}</pre></section>
}
