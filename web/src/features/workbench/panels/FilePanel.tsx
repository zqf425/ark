import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { FilePreview } from '../../../shared/contracts'
import { EmptyState } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { getFilePreview } from '../api/workbench-api'

function describe(error: unknown): string {
  if (error instanceof ApiError) return error.message
  return error instanceof Error ? error.message : String(error)
}

export function FilePanel(): JSX.Element {
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

  if (!threadId || !path) return <EmptyState title="未选择文件" hint="在 Changes 中选择文件；预览只显示本会话 read 工具已经读取的内容。" />
  if (error) return <EmptyState title="无法预览文件" hint={`${error}。先让 agent 使用 read 工具读取该文件。`} />
  if (!file) return <p className="ark-panel__hint">正在加载文件预览…</p>
  return <section className="ark-panel"><header className="ark-panel__head"><h2>{file.path}</h2><span>seq {file.source_seq}</span></header><pre className="ark-panel__code">{file.content}</pre></section>
}
