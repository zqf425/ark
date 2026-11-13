import { useEffect, useMemo, useRef, useState } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadSummary } from '../../../shared/contracts'
import { formatRelative } from '../../../shared/format'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useThreadList } from '../hooks/use-thread-list'
import { exportThread } from '../api/threads-api'
import { filterThreadsWithWorkspace, type ThreadListScope } from './thread-list-filter'
import { groupThreadsByProject, type ThreadProject } from './thread-projects'
import { ScheduleCreateDialog } from './ScheduleCreateDialog'

function ThreadRow({
  thread,
  active,
  busy,
  sourceTitle,
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
  /** Title of the source thread, when this row is a fork. */
  sourceTitle?: string
}): JSX.Element {
  const { t } = useI18n()
  const [editing, setEditing] = useState(false)
  const [title, setTitle] = useState(thread.title)
  const [menuOpen, setMenuOpen] = useState(false)
  const [menuPos, setMenuPos] = useState({ top: 0, left: 0 })
  const kebabRef = useRef<HTMLButtonElement | null>(null)
  const menuRef = useRef<HTMLDivElement | null>(null)

  /** Anchor the menu to the kebab button; fixed positioning avoids row clipping. */
  const openMenu = (): void => {
    const rect = kebabRef.current?.getBoundingClientRect()
    if (rect) setMenuPos({ top: rect.bottom + 4, left: Math.max(8, rect.right - 168) })
    setMenuOpen(true)
  }

  useEffect(() => {
    if (!menuOpen) return
    const onPointerDown = (event: MouseEvent): void => {
      const target = event.target as Node
      if (menuRef.current?.contains(target) || kebabRef.current?.contains(target)) return
      setMenuOpen(false)
    }
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setMenuOpen(false)
    }
    document.addEventListener('mousedown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('mousedown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [menuOpen])

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

  /** Close the menu, then surface failures (e.g. a failed export) visibly. */
  const runMenuAction = (action: () => Promise<void>): void => {
    setMenuOpen(false)
    void action().catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error)
      window.alert(t('thread.row.actionFailed', { message }))
    })
  }

  return (
    <div className={`ark-threadrow${active ? ' is-active' : ''}`}>
      <div className="ark-threadrow__main">
        <button type="button" className="ark-threadrow__select" onClick={onSelect} title={thread.title}>
          <span className="ark-threadrow__title">{thread.title || t('thread.row.untitled')}</span>
          <span className="ark-threadrow__meta">
            <time>{formatRelative(thread.updated_at, t)}</time>
            {typeof thread.last_seq === 'number' ? <span>seq {thread.last_seq}</span> : null}
            {thread.forked_from ? (
              <span
                className="ark-threadrow__fork"
                title={sourceTitle ? t('thread.row.forkedFrom', { title: sourceTitle }) : t('thread.row.forked')}
              >
                {sourceTitle ? t('thread.row.forkedFrom', { title: sourceTitle }) : t('thread.row.forked')}
              </span>
            ) : null}
            {thread.archived ? <span>{t('thread.row.archived')}</span> : null}
          </span>
        </button>
        <button
          type="button"
          ref={kebabRef}
          className="ark-threadrow__kebab"
          aria-label={t('thread.row.menu')}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          disabled={busy && !menuOpen}
          onClick={() => (menuOpen ? setMenuOpen(false) : openMenu())}
        >
          <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
            <circle cx="12" cy="5" r="1.9" fill="currentColor" />
            <circle cx="12" cy="12" r="1.9" fill="currentColor" />
            <circle cx="12" cy="19" r="1.9" fill="currentColor" />
          </svg>
        </button>
      </div>
      {editing ? (
        <div className="ark-threadrow__renamebar">
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
        </div>
      ) : null}
      {menuOpen ? (
        <div ref={menuRef} className="ark-threadrow__menu" role="menu" style={{ top: menuPos.top, left: menuPos.left }}>
          <button
            type="button"
            role="menuitem"
            disabled={busy}
            onClick={() => {
              setMenuOpen(false)
              setTitle(thread.title)
              setEditing(true)
            }}
          >
            {t('thread.row.rename')}
          </button>
          <button type="button" role="menuitem" disabled={busy} onClick={() => runMenuAction(onFork)}>
            {t('thread.row.fork')}
          </button>
          {thread.archived ? (
            <button type="button" role="menuitem" disabled={busy} onClick={() => runMenuAction(onRestore)}>
              {t('thread.row.restore')}
            </button>
          ) : (
            <button type="button" role="menuitem" disabled={busy} onClick={() => runMenuAction(onArchive)}>
              {t('thread.row.archive')}
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            disabled={busy}
            title={t('thread.row.exportHint')}
            onClick={() => runMenuAction(() => exportThread(thread.thread_id, thread.title))}
          >
            {t('thread.row.export')}
          </button>
          <button
            type="button"
            role="menuitem"
            className="is-danger"
            disabled={busy}
            onClick={() => runMenuAction(onDelete)}
          >
            {t('thread.row.delete')}
          </button>
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
  /** The scheduled-task entry opens a dialog; the rail itself never navigates. */
  const [scheduleOpen, setScheduleOpen] = useState(false)
  /** Collapsed project paths; the default group starts expanded with the rest. */
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set())
  const visibleThreads = useMemo(
    () => filterThreadsWithWorkspace(threads, query, scope),
    [threads, query, scope]
  )
  const groups = useMemo(() => {
    const { default: fallback, projects } = groupThreadsByProject(visibleThreads)
    const list: ThreadProject[] = []
    if (fallback.threads.length > 0) {
      list.push({ ...fallback, label: t('thread.list.defaultProject') })
    }
    // Groups are built from already-filtered rows, so every group has at least
    // one matching thread; the header itself must not be filtered by the
    // needle or a title hit would hide its project.
    list.push(...projects)
    return list
  }, [visibleThreads, t])
  const titlesById = useMemo(() => new Map(threads.map((row) => [row.thread_id as string, row.title])), [threads])
  const recentCount = useMemo(() => threads.filter((thread) => !thread.archived).length, [threads])
  const archivedCount = threads.length - recentCount

  const toggleProject = (path: string): void => {
    setCollapsed((previous) => {
      const next = new Set(previous)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  return (
    <aside className="ark-threadlist">
      <header className="ark-threadlist__head">
        <div>
          <h1 className="ark-threadlist__title">{t('thread.list.title')}</h1>
        </div>
        <div className="ark-threadlist__head-actions">
          <button
            type="button"
            className="ark-threadlist__schedule"
            title={t('thread.list.scheduleHint')}
            aria-label={t('thread.list.scheduleHint')}
            onClick={() => setScheduleOpen(true)}
          >
            <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
              <circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" strokeWidth="1.7" />
              <path
                d="M12 7.8V12l2.9 1.8"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.7"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
            <span>{t('thread.list.schedule')}</span>
          </button>
          <Button variant="primary" size="sm" disabled={busy} onClick={() => void create()}>
            {busy ? t('thread.list.creating') : t('thread.list.create')}
          </Button>
        </div>
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
          <nav className="ark-threadlist__nav" aria-label={t('thread.list.projectsAria')}>
            {groups.map((group) => {
              const isCollapsed = collapsed.has(group.path)
              return (
                <section key={group.path || '__default__'} className="ark-projectgroup">
                  <button
                    type="button"
                    className="ark-projectgroup__head"
                    aria-expanded={!isCollapsed}
                    aria-label={t('thread.list.groupToggle', { title: group.label })}
                    onClick={() => toggleProject(group.path)}
                  >
                    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false">
                      <path
                        d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2.5h9A1.5 1.5 0 0 1 21 10v7.5A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10Z"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.6"
                        strokeLinejoin="round"
                      />
                    </svg>
                    <span className="ark-projectgroup__label" title={group.path || undefined}>
                      {group.label}
                    </span>
                    {group.parent ? <span className="ark-projectgroup__parent">{group.parent}</span> : null}
                    <span className="ark-projectgroup__meta">
                      {group.path ? (
                        <span
                          className="ark-projectgroup__hint"
                          title={group.path}
                        >
                          {group.path}
                        </span>
                      ) : (
                        <span title={t('thread.list.defaultProjectHint')} />
                      )}
                      <span className="ark-projectgroup__count">
                        {t('thread.list.groupCount', { count: group.threads.length })}
                      </span>
                    </span>
                  </button>
                  {!isCollapsed ? (
                    <div className="ark-projectgroup__threads">
                      {group.threads.map((thread) => (
                        <ThreadRow
                          key={thread.thread_id}
                          thread={thread}
                          active={thread.thread_id === activeThreadId}
                          busy={busy}
                          sourceTitle={thread.forked_from ? titlesById.get(thread.forked_from) : undefined}
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
                    </div>
                  ) : null}
                </section>
              )
            })}
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

      {scheduleOpen ? <ScheduleCreateDialog onClose={() => setScheduleOpen(false)} /> : null}
    </aside>
  )
}
