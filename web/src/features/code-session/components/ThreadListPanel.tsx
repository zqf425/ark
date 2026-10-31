import { useMemo, useState } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadSummary } from '../../../shared/contracts'
import { formatRelative } from '../../../shared/format'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useThreadList } from '../hooks/use-thread-list'
import { filterThreads, type ThreadListScope } from './thread-list-filter'

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
  const { t } = useI18n()
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
        <span className="ark-threadrow__title">{thread.title || t('thread.row.untitled')}</span>
        <span className="ark-threadrow__meta">
          <time>{formatRelative(thread.updated_at, t)}</time>
          {typeof thread.last_seq === 'number' ? <span>seq {thread.last_seq}</span> : null}
          {thread.archived ? <span>{t('thread.row.archived')}</span> : null}
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
                {t('common.save')}
              </button>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => setEditing(false)}>
                {t('common.cancel')}
              </button>
            </>
          ) : (
            <>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => setEditing(true)}>
                {t('thread.row.rename')}
              </button>
              <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onFork()}>
                {t('thread.row.fork')}
              </button>
              {thread.archived ? (
                <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onRestore()}>{t('thread.row.restore')}</button>
              ) : (
                <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void onArchive()}>{t('thread.row.archive')}</button>
              )}
              <button type="button" className="ark-linkbtn ark-linkbtn--danger" disabled={busy} onClick={() => void onDelete()}>
                {t('thread.row.delete')}
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
  const { t } = useI18n()
  const threads = useCodeSessionStore((s) => s.threads)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const listStatus = useCodeSessionStore((s) => s.listStatus)
  const listError = useCodeSessionStore((s) => s.listError)
  const { create, select, refresh, rename, archive, restore, remove, fork, busy } = useThreadList()
  const [scope, setScope] = useState<ThreadListScope>('recent')
  const [query, setQuery] = useState('')
  const visibleThreads = useMemo(() => filterThreads(threads, query, scope), [threads, query, scope])
  const recentCount = useMemo(() => threads.filter((thread) => !thread.archived).length, [threads])
  const archivedCount = threads.length - recentCount

  return (
    <aside className="ark-threadlist">
      <header className="ark-threadlist__head">
        <div>
          <h1 className="ark-threadlist__title">{t('thread.list.title')}</h1>
        </div>
        <Button variant="primary" size="sm" disabled={busy} onClick={() => void create()}>
          {busy ? t('thread.list.creating') : t('thread.list.create')}
        </Button>
      </header>

      <div className="ark-threadlist__controls">
        <label className="ark-threadlist__search">
          <span className="ark-threadlist__search-icon" aria-hidden="true">⌕</span>
          <input
            type="search"
            value={query}
            placeholder={t('thread.list.search')}
            aria-label={t('thread.list.search')}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <div className="ark-threadlist__scope" role="group" aria-label={t('thread.list.scopeAria')}>
          <button type="button" className={scope === 'recent' ? 'is-active' : ''} aria-pressed={scope === 'recent'} onClick={() => setScope('recent')}>
            {t('thread.list.scopeRecent')} <span>{recentCount}</span>
          </button>
          <button type="button" className={scope === 'archived' ? 'is-active' : ''} aria-pressed={scope === 'archived'} onClick={() => setScope('archived')}>
            {t('thread.list.scopeArchived')} <span>{archivedCount}</span>
          </button>
        </div>
      </div>

      <div className="ark-threadlist__body">
        {listStatus === 'loading' && threads.length === 0 ? (
          <p className="ark-threadlist__hint">{t('thread.list.loading')}</p>
        ) : null}
        {listStatus === 'error' ? (
          <div className="ark-threadlist__hint is-error">
            <span>{listError}</span>
            <button type="button" className="ark-linkbtn" onClick={() => void refresh()}>
              {t('common.retry')}
            </button>
          </div>
        ) : null}
        {listStatus === 'ready' && threads.length === 0 ? (
          <EmptyState title={t('thread.list.emptyTitle')} hint={t('thread.list.emptyHint')} />
        ) : null}
        {listStatus === 'ready' && threads.length > 0 && visibleThreads.length === 0 ? (
          <EmptyState
            title={t('thread.list.noMatchTitle')}
            hint={query ? t('thread.list.noMatchQueryHint') : t('thread.list.noMatchScopeHint')}
          />
        ) : null}

        <ErrorBoundary label={t('thread.list.boundary')}>
          <nav className="ark-threadlist__nav">
            {visibleThreads.map((thread) => (
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
                  if (await archive(thread.thread_id)) setScope('archived')
                }}
                onRestore={async () => {
                  if (await restore(thread.thread_id)) setScope('recent')
                }}
                onDelete={async () => {
                  const name = thread.title || t('thread.row.untitled')
                  if (window.confirm(t('thread.row.deleteConfirm', { title: name }))) {
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
          {t('thread.list.refresh')}
        </button>
        <span className="ark-threadlist__count">
          {t('thread.list.count', { visible: visibleThreads.length, total: threads.length })}
        </span>
      </footer>
    </aside>
  )
}
