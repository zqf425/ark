import type { KeyboardEvent, ReactNode } from 'react'
import { Button } from '../../../shared/ui'
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
      aria-label={side === 'left' ? '调整会话列表宽度' : '调整右侧面板宽度'}
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
 * Three-column workbench (design §7.10.2): thread rail, stream, panel slot.
 * Widths live in the workbench slice so layout survives thread switches.
 */
export function WorkbenchLayout({ left, center, right, top }: WorkbenchLayoutProps): JSX.Element {
  const leftWidth = useCodeSessionStore((s) => s.leftWidth)
  const rightWidth = useCodeSessionStore((s) => s.rightWidth)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const setRightWidth = useCodeSessionStore((s) => s.setRightWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)

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
          <div className="ark-col__tools">
            <Button variant="ghost" size="sm" onClick={toggleLeft}>
              {leftCollapsed ? '显示会话列表' : '隐藏会话列表'}
            </Button>
          </div>
          {center}
        </div>

        <Resizer side="right" width={rightWidth} setWidth={setRightWidth} />
        <div className="ark-col ark-col--right">{right}</div>
      </div>
    </div>
  )
}
