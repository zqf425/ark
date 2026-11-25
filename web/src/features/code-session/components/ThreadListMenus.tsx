import { useEffect, useRef, useState } from 'react'
import { Icon, useToast } from '../../../shared/ui'
import type { ThreadSummary } from '../../../shared/contracts'
import { formatRelative } from '../../../shared/format'
import { useI18n } from '../../../shared/i18n'
import { activityKeyOf } from '../state/threads-slice'
import { exportThread } from '../api/threads-api'
import type { ThreadProject } from './thread-projects'
import { workspaceBasename } from './thread-projects'

const MENU_GAP = 4
const MENU_EDGE = 8
const MENU_WIDTH = 184

type Anchor = { top: number; bottom: number; right: number }
type MenuPosition = { top: number | null; bottom: number | null; left: number; anchor?: Anchor }

/** Measure the rendered menu so the bottom row's delete item stays clickable. */
function useMenuPosition(menuRef: React.RefObject<HTMLDivElement | null>, anchor: Anchor | null): MenuPosition {
  const [height, setHeight] = useState(() => typeof window === 'undefined' ? 184 : Math.min(184, window.innerHeight - 16))
  useEffect(() => {
    const menu = menuRef.current
    if (!menu || !anchor) return
    const measure = (): void => {
      const next = menu.getBoundingClientRect().height || menu.scrollHeight
      if (next && next !== height) setHeight(next)
    }
    measure()
    const observer = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : null
    observer?.observe(menu)
    return () => observer?.disconnect()
  }, [menuRef, anchor, height])
  if (!anchor) return { top: 0, bottom: null, left: 0 }
  const left = Math.max(MENU_EDGE, Math.min(anchor.right - MENU_WIDTH, window.innerWidth - MENU_EDGE - MENU_WIDTH))
  return anchor.bottom + MENU_GAP + height > window.innerHeight - MENU_EDGE
    ? { top: null, bottom: Math.max(MENU_EDGE, window.innerHeight - anchor.top + MENU_GAP), left }
    : { top: anchor.bottom + MENU_GAP, bottom: null, left }
}

function MenuSurface({ position, children, onDismiss }: { position: MenuPosition; children: React.ReactNode; onDismiss: () => void }): JSX.Element {
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const onPointer = (event: MouseEvent): void => {
      const target = event.target as Node
      if (!ref.current?.contains(target) && !(target as HTMLElement).closest?.('[data-menu-trigger]')) onDismiss()
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') onDismiss()
    }
    document.addEventListener('mousedown', onPointer)
    document.addEventListener('keydown', onKey)
    return () => { document.removeEventListener('mousedown', onPointer); document.removeEventListener('keydown', onKey) }
  }, [onDismiss])
  // Re-measure after mount: layout (font/locale) decides the real height.
  const final = useMenuPosition(ref, position.anchor ?? null)
  return (
    <div
      ref={ref}
      className="ark-threadrow__menu"
      role="menu"
      style={final.top === null ? { bottom: final.bottom ?? 0, left: final.left } : { top: final.top ?? 0, left: final.left }}
    >
      {children}
    </div>
  )
}

export type ThreadRowMenuProps = {
  anchor: Anchor | null
  thread: ThreadSummary
  busy: boolean
  sourceTitle?: string
  onRename: () => void
  onFork: () => Promise<void>
  onArchive: () => Promise<void>
  onRestore: () => Promise<void>
  onDelete: () => Promise<void>
  onClose: () => void
}

/** Fixed-position row actions; measured height keeps the last item clickable. */
export function ThreadRowMenu({ anchor, thread, busy, sourceTitle, onRename, onFork, onArchive, onRestore, onDelete, onClose }: ThreadRowMenuProps): JSX.Element | null {
  const { t } = useI18n()
  const toast = useToast()
  if (!anchor) return null
  const run = (action: () => Promise<void>): void => {
    onClose()
    void action().catch((error: unknown) => {
      toast.push('error', t('thread.row.actionFailed', { message: error instanceof Error ? error.message : String(error) }))
    })
  }
  return (
    <MenuSurface position={{ top: 0, bottom: null, left: 0, anchor }} onDismiss={onClose}>
      <button type="button" role="menuitem" disabled={busy} onClick={() => { onClose(); onRename() }}>{t('thread.row.rename')}</button>
      <button type="button" role="menuitem" disabled={busy} onClick={() => run(onFork)}>{t('thread.row.fork')}</button>
      {thread.archived ? (
        <button type="button" role="menuitem" disabled={busy} onClick={() => run(onRestore)}>{t('thread.row.restore')}</button>
      ) : (
        <button type="button" role="menuitem" disabled={busy} onClick={() => run(onArchive)}>{t('thread.row.archive')}</button>
      )}
      <button type="button" role="menuitem" disabled={busy} title={t('thread.row.exportHint')} onClick={() => run(() => exportThread(thread.thread_id, thread.title))}>{t('thread.row.export')}</button>
      <button type="button" role="menuitem" className="is-danger" disabled={busy} onClick={() => run(onDelete)}>{t('thread.row.delete')}</button>
      {sourceTitle ? <div className="ark-threadrow__menu-hint" title={sourceTitle}>{t('thread.row.forkedFrom', { title: sourceTitle })}</div> : null}
    </MenuSurface>
  )
}

/** Row menu anchor from a kebab button, with viewport-safe position. */
export function menuAnchorOf(element: HTMLElement | null): Anchor {
  const rect = element?.getBoundingClientRect()
  return rect ? { top: rect.top, bottom: rect.bottom, right: rect.right } : { top: 0, bottom: 0, right: MENU_EDGE + MENU_WIDTH }
}

/** Project group overflow menu: create and bulk delete live behind one `⋯`. */
export function ProjectGroupMenu({ anchor, group, busy, onCreate, onDelete, onClose }: {
  anchor: Anchor | null
  group: ThreadProject
  busy: boolean
  onCreate: () => void
  onDelete: () => void
  onClose: () => void
}): JSX.Element | null {
  const { t } = useI18n()
  if (!anchor) return null
  const title = group.label || t('thread.list.defaultProject')
  return (
    <MenuSurface position={{ top: 0, bottom: null, left: 0, anchor }} onDismiss={onClose}>
      <button type="button" role="menuitem" disabled={busy || !group.path} onClick={() => { onClose(); onCreate() }}>
        {t('thread.list.groupCreate', { title })}
      </button>
      <button type="button" role="menuitem" className="is-danger" disabled={busy || !group.path || group.threads.length === 0} onClick={() => { onClose(); onDelete() }}>
        {t('thread.list.groupDelete', { title })}
      </button>
    </MenuSurface>
  )
}

/** Compact status dot used by running rows. */
export function RunningDot(): JSX.Element {
  const { t } = useI18n()
  return (
    <span className="ark-threadrow__running" role="status" aria-label={t('thread.row.running')} title={t('thread.row.running')}>
      <Icon name="clock" size={11} />
      <span>{t('thread.row.runningShort')}</span>
    </span>
  )
}

/** Meta line priority: time > fork > workspace on narrow rails. */
export function ThreadMeta({ thread, sourceTitle }: { thread: ThreadSummary; sourceTitle?: string }): JSX.Element {
  const { t } = useI18n()
  return (
    <span className="ark-threadrow__meta">
      <time>{formatRelative(activityKeyOf(thread), t)}</time>
      {sourceTitle ? <span className="ark-threadrow__fork" title={t('thread.row.forkedFrom', { title: sourceTitle })}><Icon name="branch" size={11} />{t('thread.row.forkedFrom', { title: sourceTitle })}</span> : null}
      {thread.workspace ? <span className="ark-threadrow__workspace" title={thread.workspace}>{workspaceBasename(thread.workspace)}</span> : null}
      {thread.archived ? <span className="ark-threadrow__archived">{t('thread.row.archived')}</span> : null}
    </span>
  )
}
