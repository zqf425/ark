import { type KeyboardEvent, type ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { resizerKeyDelta, usePanelResizer } from './use-panel-resizer'
import '../workbench.css'

/** A draggable 1px gutter that is also keyboard operable. */
function Resizer({
  width,
  setWidth
}: {
  width: number
  setWidth: (width: number) => void
}): JSX.Element {
  const { t } = useI18n()
  const { onPointerDown } = usePanelResizer(width, setWidth)
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const delta = resizerKeyDelta(event.key)
    if (delta === null) return
    event.preventDefault()
    setWidth(width + delta)
  }

  return (
    <div
      className="ark-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label={t('workbench.layout.resizeLeft')}
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
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  return (
    <div
      className="console-workbench"
      style={{ '--ark-left': `${leftWidth}px` } as React.CSSProperties}
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
          <div className="console-col console-col--left">{left}</div>
          <Resizer width={leftWidth} setWidth={setLeftWidth} />
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

      <aside
        id="ark-context-drawer"
        className="console-inspector"
        hidden={rightCollapsed}
        aria-label={t('console.inspector.title')}
      >
        <div className="console-inspector__head">
          <span className="console-inspector__title">{t('console.inspector.title')}</span>
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
