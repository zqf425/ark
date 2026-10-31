import type { KeyboardEvent, ReactNode } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { resizerKeyDelta, usePanelResizer } from './use-panel-resizer'
import '../workbench.css'

/** A draggable 1px gutter that is also keyboard operable. */
function Resizer({
  side,
  width,
  setWidth
}: {
  side: 'left' | 'right'
  width: number
  setWidth: (width: number) => void
}): JSX.Element {
  const { t } = useI18n()
  const { onPointerDown } = usePanelResizer(side, width, setWidth)
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const delta = resizerKeyDelta(event.key, side)
    if (delta === null) return
    event.preventDefault()
    setWidth(width + delta)
  }

  return (
    <div
      className="ark-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label={side === 'left' ? t('workbench.layout.resizeLeft') : t('workbench.layout.resizeRight')}
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
  top: ReactNode
}

/**
 * Conversation-first workbench: task rail and wide stream are the default;
 * the context projection is a user-opened drawer. Widths stay local to the
 * layout slice and never alter thread or runtime state.
 */
export function WorkbenchLayout({ left, center, right, top }: WorkbenchLayoutProps): JSX.Element {
  const { t } = useI18n()
  const leftWidth = useCodeSessionStore((s) => s.leftWidth)
  const rightWidth = useCodeSessionStore((s) => s.rightWidth)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const setRightWidth = useCodeSessionStore((s) => s.setRightWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  return (
    <div className="ark-shell">
      {top}
      <div className="ark-cols" style={{ '--ark-left': `${leftWidth}px`, '--ark-right': `${rightWidth}px` } as React.CSSProperties}>
        {leftCollapsed ? null : (
          <>
            <div className="ark-col ark-col--left">{left}</div>
            <Resizer side="left" width={leftWidth} setWidth={setLeftWidth} />
          </>
        )}

        <div className="ark-col ark-col--center">
          <div className="ark-workbench-tools" aria-label={t('workbench.layout.aria')}>
            <Button
              variant="ghost"
              size="sm"
              aria-pressed={!leftCollapsed}
              title={leftCollapsed ? t('workbench.layout.showThreads') : t('workbench.layout.hideThreads')}
              onClick={toggleLeft}
            >
              {leftCollapsed ? t('workbench.layout.threads') : t('workbench.layout.collapse')}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-pressed={!rightCollapsed}
              title={rightCollapsed ? t('workbench.layout.openContext') : t('workbench.layout.closeContext')}
              onClick={toggleRight}
            >
              {rightCollapsed ? t('workbench.layout.context') : t('workbench.layout.close')}
            </Button>
          </div>
          {center}
        </div>

        {rightCollapsed ? null : <Resizer side="right" width={rightWidth} setWidth={setRightWidth} />}
        <div className="ark-col ark-col--right" hidden={rightCollapsed}>
          {right}
        </div>
      </div>
    </div>
  )
}
