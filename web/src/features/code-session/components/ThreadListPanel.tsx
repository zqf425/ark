import { useState } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadSummary } from '../../../shared/contracts'
import { formatRelative } from '../../../shared/format'
import { useCodeSessionStore } from '../state/store'
import { useThreadList } from '../hooks/use-thread-list'

function ThreadRow({
  thread,
  active,
  busy,
  onSelect,
  onRename,
  onArchive,
  onRestore,
  onFork,
  onDelete
}: {
  thread: ThreadSummary
  active: boolean
  busy: boolean
  onSelect: () => void
  onRename: (title: string) => Promise<void>
  onArchive: () => Promise<void>
  onRestore: () => Promise<void>
  onFork: () => Promise<void>
  onDelete: () => Promise<void>
}): JSX.Element {
  const [editing, setEditing] = useState(false)
  const [title, setTitle] = useState(thread.title)

  const submitRename = async (): Promise<void> => {
    const next = title.trim()
    if (!next || next === thread.title) {
      setEditing(false)
      setTitle(thread.title)
      return
    }
    await onRename(next)
    setEditing(false)
  }

  return (
    <div className={`ark-threadrow${active ? ' is-active' : ''}`}>
      <button type="button" className="ark-threadrow__select" onClick={onSelect} title={thread.title}>
        <span className="ark-threadrow__title">{thread.title || '未命名会话'}</span>
        <span className="ark-threadrow__meta">
          <time>{formatRelative(thread.updated_at)}</time>
          {typeof thread.last_seq === 'number' ? <span>seq {thread.last_seq}</span> : null}
          {thread.archived ? <span>已归档</span> : null}
        </span>
      </button>
      {active ? (
        <div className="ark-threadrow__actions">
          {editing ? (
            <>
              <input
                className="ark-threadrow__rename"
                value={title}
                disabled={busy}
                autoFocus
                onChange={(event) => setTitle(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') void submitRename()
                  if (event.key === 'Escape') {
                    setEditing(false)
                    setTitle(thread.title)
                  }
                }}
              />
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void submitRename()}>
                保存
              </button>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => setEditing(false)}>
                取消
              </button>
            </>
          ) : (
            <>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => setEditing(true)}>
                重命名
              </button>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onFork()}>
                分叉
              </button>
              {thread.archived ? (
                <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onRestore()}>恢复</button>
              ) : (
                <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onArchive()}>归档</button>
              )}
              <button type="button" className="ark-linkbtn ark-linkbtn--danger" disabled={busy} onClick={() => void onDelete()}>
                删除
              </button>
            </>
          )}
        </div>
      ) : null}
    </div>
  )
}

/** Left column: thread list plus the only "new thread" action. */
export function ThreadListPanel(): JSX.Element {
  const threads = useCodeSessionStore((s) => s.threads)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const listStatus = useCodeSessionStore((s) => s.listStatus)
  const listError = useCodeSessionStore((s) => s.listError)
  const { create, select, refresh, rename, archive, restore, remove, fork, busy } = useThreadList()

  return (
    <aside className="ark-threadlist">
      <header className="ark-threadlist__head">
        <span className="ark-threadlist__title">会话</span>
        <Button variant="primary" size="sm" disabled={busy} onClick={() => void create()}>
          {busy ? '新建中…' : '新建'}
        </Button>
      </header>

      <div className="ark-threadlist__body">
        {listStatus === 'loading' && threads.length === 0 ? (
          <p className="ark-threadlist__hint">载入线程列表…</p>
        ) : null}
        {listStatus === 'error' ? (
          <div className="ark-threadlist__hint is-error">
            <span>{listError}</span>
            <button type="button" className="ark-linkbtn" onClick={() => void refresh()}>
              重试
            </button>
          </div>
        ) : null}
        {listStatus === 'ready' && threads.length === 0 ? (
          <EmptyState title="还没有会话" hint="点击“新建”开始。" />
        ) : null}

        <ErrorBoundary label="线程列表渲染失败">
          <nav className="ark-threadlist__nav">
            {threads.map((thread) => (
              <ThreadRow
                key={thread.thread_id}
                thread={thread}
                active={thread.thread_id === activeThreadId}
                busy={busy}
                onSelect={() => select(thread.thread_id)}
                onRename={async (title) => {
                  await rename(thread.thread_id, title)
                }}
                onFork={async () => {
                  await fork(thread.thread_id)
                }}
                onArchive={async () => {
                  await archive(thread.thread_id)
                }}
                onRestore={async () => {
                  await restore(thread.thread_id)
                }}
                onDelete={async () => {
                  if (window.confirm(`删除会话“${thread.title || '未命名会话'}”？此操作不可恢复。`)) {
                    await remove(thread.thread_id)
                  }
                }}
              />
            ))}
          </nav>
        </ErrorBoundary>
      </div>

      <footer className="ark-threadlist__foot">
        <button type="button" className="ark-linkbtn" onClick={() => void refresh()}>
          刷新列表
        </button>
        <span className="ark-threadlist__count">{threads.length} 个</span>
      </footer>
    </aside>
  )
}
