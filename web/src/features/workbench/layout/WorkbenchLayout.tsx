import { type KeyboardEvent, type ReactNode } from 'react'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { resizerKeyDelta, usePanelResizer, type ResizerAnchor } from './use-panel-resizer'
import '../workbench.css'

/**
 * A draggable 1px gutter that is also keyboard operable. `anchor` decides
 * which direction "wider" is: a right-anchored panel (the inspector) grows
 * when the user drags its left edge toward the left.
 */
function Resizer({
  width,
  setWidth,
  anchor,
  label,
  className
}: {
  width: number
  setWidth: (width: number) => void
  anchor: ResizerAnchor
  label: MessageKey
  className: string
}): JSX.Element {
  const { t } = useI18n()
  const { onPointerDown } = usePanelResizer(width, setWidth, anchor)
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const delta = resizerKeyDelta(event.key, anchor)
    if (delta === null) return
    event.preventDefault()
    setWidth(width + delta)
  }

  return (
    <div
      className={`ark-resizer${className ? ` ${className}` : ''}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={t(label)}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
    />
  )
}

export type WorkbenchLayoutProps = {
  left: ReactNode
  center: ReactNode
  right: ReactNode
}

/**
 * Workbench scene of the approved ARK Console shell: thread list, conversation
 * and the resident inspector side by side. The inspector is a column, not the
 * old floating drawer — it stays mounted while collapsed via `hidden` so panel
 * drafts and server read models survive a toggle.
 */
export function WorkbenchLayout({ left, center, right }: WorkbenchLayoutProps): JSX.Element {
  const { t } = useI18n()
  const leftWidth = useCodeSessionStore((s) => s.leftWidth)
  const rightWidth = useCodeSessionStore((s) => s.rightWidth)
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const setRightWidth = useCodeSessionStore((s) => s.setRightWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  return (
    <div
      className="console-workbench"
      style={{ '--ark-left': `${leftWidth}px`, '--ark-right': `${rightWidth}px` } as React.CSSProperties}
    >
      {leftCollapsed ? (
        <button
          type="button"
          className="console-sidebar-tab"
          aria-label={t('workbench.layout.showThreads')}
          title={t('workbench.layout.showThreads')}
          onClick={toggleLeft}
        >
          ‹
        </button>
      ) : (
        <>
          <div className="console-col console-col--left">
            {left}
            {/* 收起入口放在列内右下角：与检查器折叠按钮同视觉语言，避开列表头部的
                定时任务/新建动作，也不与行级 kebab 菜单争位。 */}
            <button
              type="button"
              className="console-sidebar-collapse"
              title={t('workbench.layout.hideThreads')}
              aria-label={t('workbench.layout.hideThreads')}
              aria-expanded
              onClick={toggleLeft}
            >
              ‹
            </button>
          </div>
          <Resizer
            width={leftWidth}
            setWidth={setLeftWidth}
            anchor="left"
            label="workbench.layout.resizeLeft"
            className="console-left-resizer"
          />
        </>
      )}

      <div className="console-col console-col--center">
        {rightCollapsed ? (
          <button
            type="button"
            className="console-inspector-toggle"
            title={t('console.inspector.expand')}
            aria-label={t('console.inspector.expand')}
            aria-expanded={false}
            onClick={toggleRight}
          >
            <Icon name="panel" size={14} />
          </button>
        ) : null}
        {center}
      </div>

      {!rightCollapsed ? (
        <Resizer
          width={rightWidth}
          setWidth={setRightWidth}
          anchor="right"
          label="workbench.layout.resizeRight"
          className="console-inspector-resizer"
        />
      ) : null}
      <aside
        id="ark-context-drawer"
        className="console-inspector"
        hidden={rightCollapsed}
        aria-label={t('console.inspector.title')}
      >
        <div className="console-inspector__head">
          <span className="console-inspector__title">{t('console.inspector.title')}</span>
          <InspectorSessionSummary />
          <span className="console-inspector__grow" />
          <button
            type="button"
            className="console-iconbtn"
            title={t('console.inspector.collapse')}
            aria-label={t('console.inspector.collapse')}
            aria-expanded
            onClick={toggleRight}
          >
            <Icon name="panel" size={14} />
          </button>
        </div>
        <div className="console-inspector__body">{right}</div>
      </aside>
    </div>
  )
}


/** Current-session context in the inspector header; reads store only. */
function InspectorSessionSummary(): JSX.Element | null {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((s) => s.activeThreadId)
  const threads = useCodeSessionStore((s) => s.threads)
  if (!threadId) return null
  const thread = threads.find((row) => row.thread_id === threadId)
  if (!thread) return <span className="console-inspector__summary">{t('workbench.noThreadTitle')}</span>
  return (
    <span className="console-inspector__summary" title={thread.title}>
      {thread.title || t('thread.row.untitled')}
    </span>
  )
}
