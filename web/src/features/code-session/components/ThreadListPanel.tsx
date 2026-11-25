import { useEffect, useMemo, useRef, useState } from 'react'
import { Button, EmptyState, ErrorBoundary, Icon, useToast } from '../../../shared/ui'
import type { ThreadSummary } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useThreadList } from '../hooks/use-thread-list'
import { useThreadRunning } from '../hooks/use-thread-running'
import { getWorkspaceView } from '../../settings/api/workspace-api'
import { filterThreadsWithWorkspace, type ThreadListScope } from './thread-list-filter'
import { groupThreadsByProject, workspaceBasename, type ThreadProject } from './thread-projects'
import { RunningDot, ThreadMeta, ThreadRowMenu, menuAnchorOf } from './ThreadListMenus'
import { ScheduleCreateDialog } from './ScheduleCreateDialog'

type MenuAnchor = ReturnType<typeof menuAnchorOf>

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
  sourceTitle?: string
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
  const [menuAnchor, setMenuAnchor] = useState<MenuAnchor | null>(null)

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
      <div className="ark-threadrow__main">
        <button type="button" className="ark-threadrow__select" onClick={onSelect} title={thread.title}>
          <span className="ark-threadrow__title">
            {thread.running ? <RunningDot /> : null}
            {thread.title || t('thread.row.untitled')}
          </span>
          <ThreadMeta thread={thread} sourceTitle={sourceTitle} />
        </button>
        <button
          type="button"
          data-menu-trigger
          className="ark-threadrow__kebab"
          aria-label={t('thread.row.menu')}
          aria-haspopup="menu"
          aria-expanded={menuAnchor !== null}
          disabled={busy && menuAnchor === null}
          onClick={(event) => setMenuAnchor((current) => current ? null : menuAnchorOf(event.currentTarget))}
        >
          <Icon name="more" size={14} />
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
              if (event.key === 'Escape') { setEditing(false); setTitle(thread.title) }
            }}
          />
          <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void submitRename()}>{t('common.save')}</button>
          <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => setEditing(false)}>{t('common.cancel')}</button>
        </div>
      ) : null}
      {menuAnchor ? (
        <ThreadRowMenu
          anchor={menuAnchor}
          thread={thread}
          busy={busy}
          sourceTitle={sourceTitle}
          onRename={() => { setTitle(thread.title); setEditing(true) }}
          onFork={onFork}
          onArchive={onArchive}
          onRestore={onRestore}
          onDelete={onDelete}
          onClose={() => setMenuAnchor(null)}
        />
      ) : null}
    </div>
  )
}

/** Left column: thread list, search, scope and the only "new thread" action. */
export function ThreadListPanel(): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const threads = useCodeSessionStore((s) => s.threads)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const listStatus = useCodeSessionStore((s) => s.listStatus)
  const listError = useCodeSessionStore((s) => s.listError)
  const { create, deleteProject, select, refresh, rename, archive, restore, remove, fork, busy } = useThreadList()
  useThreadRunning(activeThreadId)
  const [scope, setScope] = useState<ThreadListScope>('recent')
  const [query, setQuery] = useState('')
  const [scheduleOpen, setScheduleOpen] = useState(false)
  const [activeWorkspace, setActiveWorkspace] = useState('')
  const searchRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    let cancelled = false
    void getWorkspaceView().then((view) => {
      if (!cancelled) setActiveWorkspace(view.active?.path ?? '')
    }).catch(() => undefined)
    return () => { cancelled = true }
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const target = event.target as HTMLElement | null
      const typing = target?.tagName === 'INPUT' || target?.tagName === 'TEXTAREA' || target?.isContentEditable
      if (event.key === '/' && !typing && !event.metaKey && !event.ctrlKey && !event.altKey) {
        event.preventDefault()
        searchRef.current?.focus()
      }
      if (event.key === 'Escape' && document.activeElement === searchRef.current) {
        setQuery('')
        searchRef.current?.blur()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])

  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set())
  const visibleThreads = useMemo(() => filterThreadsWithWorkspace(threads, query, scope), [threads, query, scope])
  const groups = useMemo(() => {
    const { default: fallback, projects } = groupThreadsByProject(visibleThreads, activeWorkspace)
    const list: ThreadProject[] = []
    if (fallback.threads.length > 0) list.push({ ...fallback, label: t('thread.list.defaultProject') })
    list.push(...projects)
    return list
  }, [visibleThreads, activeWorkspace, t])
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

  const handleProjectCreate = async (path: string): Promise<void> => {
    const created = await create(path)
    if (created) toast.push('success', t('thread.list.groupCreateDone', { title: workspaceBasename(path) }))
  }

  const handleProjectDelete = async (group: ThreadProject): Promise<void> => {
    const path = group.path.trim()
    if (!path || group.threads.length === 0) return
    const title = group.label || t('thread.list.defaultProject')
    if (!window.confirm(t('thread.list.groupDeleteConfirm', { title, count: group.threads.length }))) return
    const response = await deleteProject(path)
    if (!response) return
    if (response.skipped.length > 0) {
      toast.push('info', t('thread.list.groupDeleteSkipped', { deleted: response.deleted.length, skipped: response.skipped.length }))
    } else if (response.deleted.length > 0) {
      toast.push('success', t('thread.list.groupDeleteDone', { count: response.deleted.length }))
    }
  }

  return (
    <aside className="ark-threadlist">
      <header className="ark-threadlist__head">
        <h1 className="ark-threadlist__title">{t('thread.list.title')}</h1>
        <div className="ark-threadlist__head-actions">
          {query ? <span className="ark-threadlist__count">{t('thread.list.matches', { count: visibleThreads.length })}</span> : null}
          <button
            type="button"
            className="ark-threadlist__iconbtn"
            title={t('thread.list.scheduleHint')}
            aria-label={t('thread.list.scheduleHint')}
            onClick={() => setScheduleOpen(true)}
          >
            <Icon name="clock" size={14} />
          </button>
          <button
            type="button"
            className="ark-threadlist__iconbtn"
            title={t('thread.list.refresh')}
            aria-label={t('thread.list.refresh')}
            onClick={() => void refresh()}
          >
            <Icon name="refresh" size={14} />
          </button>
          <Button variant="primary" size="sm" disabled={busy} onClick={() => void create()}>
            {busy ? t('thread.list.creating') : t('thread.list.create')}
          </Button>
        </div>
      </header>

      <div className="ark-threadlist__controls">
        <label className="ark-threadlist__search">
          <Icon name="search" size={13} />
          <input
            ref={searchRef}
            type="search"
            value={query}
            placeholder={t('thread.list.search')}
            aria-label={t('thread.list.search')}
            onChange={(event) => setQuery(event.target.value)}
          />
          <span className="ark-threadlist__shortcut">{t('thread.list.searchShortcut')}</span>
        </label>
        <div className="ark-threadlist__scope" role="group" aria-label={t('thread.list.scopeAria')}>
          <button type="button" className={scope === 'recent' ? 'is-active' : ''} aria-pressed={scope === 'recent'} onClick={() => setScope('recent')}>
            {t('thread.list.scopeRecent')} <span>{recentCount}</span>
          </button>
          <button type="button" className={scope === 'archived' ? 'is-active' : ''} aria-pressed={scope === 'archived'} onClick={() => setScope('archived')}>
            {t('thread.list.scopeArchived')} <span>{archivedCount}</span>
          </button>
          <button type="button" className="ark-threadlist__refresh" title={t('thread.list.refresh')} aria-label={t('thread.list.refresh')} onClick={() => void refresh()}>
            <Icon name="refresh" size={13} />
          </button>
        </div>
      </div>

      <div className="ark-threadlist__body">
        {listStatus === 'loading' && threads.length === 0 ? <p className="ark-threadlist__hint">{t('thread.list.loading')}</p> : null}
        {listStatus === 'error' ? (
          <div className="ark-threadlist__hint is-error">
            <span>{listError}</span>
            <button type="button" className="ark-linkbtn" onClick={() => void refresh()}>{t('common.retry')}</button>
          </div>
        ) : null}
        {listStatus === 'ready' && threads.length === 0 ? <EmptyState title={t('thread.list.emptyTitle')} hint={t('thread.list.emptyHint')} /> : null}
        {listStatus === 'ready' && threads.length > 0 && visibleThreads.length === 0 ? (
          <EmptyState title={t('thread.list.noMatchTitle')} hint={query ? t('thread.list.noMatchQueryHint') : t('thread.list.noMatchScopeHint')} />
        ) : null}

        <ErrorBoundary label={t('thread.list.boundary')}>
          <nav className="ark-threadlist__nav" aria-label={t('thread.list.projectsAria')}>
            {groups.map((group) => {
              const isCollapsed = collapsed.has(group.path)
              return (
                <section key={group.path || '__default__'} className="ark-projectgroup">
                  <div className="ark-projectgroup__row">
                    <button type="button" className="ark-projectgroup__head" aria-expanded={!isCollapsed} aria-label={t('thread.list.groupToggle', { title: group.label })} onClick={() => toggleProject(group.path)}>
                      <Icon name="folder" size={14} />
                      <span className="ark-projectgroup__label" title={group.path || undefined}>{group.label}</span>
                      <span className="ark-projectgroup__meta">
                        <span className="ark-projectgroup__count">{t('thread.list.groupCount', { count: group.threads.length })}</span>
                      </span>
                    </button>
                    {group.path ? (
                      <span className="ark-projectgroup__actions">
                        <button
                          type="button"
                          className="ark-projectgroup__action"
                          title={t('thread.list.groupCreate', { title: group.label })}
                          aria-label={t('thread.list.groupCreate', { title: group.label })}
                          disabled={busy}
                          onClick={() => void handleProjectCreate(group.path)}
                        >
                          <Icon name="plus" size={13} />
                        </button>
                        <button
                          type="button"
                          className="ark-projectgroup__action is-danger"
                          title={t('thread.list.groupDelete', { title: group.label })}
                          aria-label={t('thread.list.groupDelete', { title: group.label })}
                          disabled={busy || group.threads.length === 0}
                          onClick={() => void handleProjectDelete(group)}
                        >
                          <Icon name="archive" size={13} />
                        </button>
                      </span>
                    ) : null}
                  </div>
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
                          onRename={async (title) => { await rename(thread.thread_id, title) }}
                          onFork={async () => { await fork(thread.thread_id) }}
                          onArchive={async () => { if (await archive(thread.thread_id)) setScope('archived') }}
                          onRestore={async () => { if (await restore(thread.thread_id)) setScope('recent') }}
                          onDelete={async () => {
                            const name = thread.title || t('thread.row.untitled')
                            if (window.confirm(t('thread.row.deleteConfirm', { title: name }))) await remove(thread.thread_id)
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

      {scheduleOpen ? <ScheduleCreateDialog onClose={() => setScheduleOpen(false)} /> : null}
    </aside>
  )
}
