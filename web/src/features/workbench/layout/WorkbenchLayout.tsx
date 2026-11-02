import type { KeyboardEvent, ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useRuntimeStatus } from '../../code-session/hooks/use-runtime-status'
import { ViewSwitch } from '../components/ViewSwitch'
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
}

/**
 * Conversation-first workbench, no global top bar: the thread rail is a
 * full-height sidebar whose footer carries the runtime status and the view
 * switch, and the context projection is a floating drawer over the canvas.
 * Widths stay local to the layout slice and never alter thread or runtime
 * state.
 */
export function WorkbenchLayout({ left, center, right }: WorkbenchLayoutProps): JSX.Element {
  const { t } = useI18n()
  const leftWidth = useCodeSessionStore((s) => s.leftWidth)
  const rightWidth = useCodeSessionStore((s) => s.rightWidth)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  // The sidebar footer owns liveness: the same store slices the old top bar
  // read, rendered as one compact row instead of a full-width banner.
  const online = useCodeSessionStore((s) => s.online)
  const healthError = useCodeSessionStore((s) => s.healthError)
  const { refresh } = useRuntimeStatus()
  const onlineLabel =
    online === 'online'
      ? t('status.online.online')
      : online === 'offline'
        ? t('status.online.offline')
        : t('status.online.unknown')
  const refreshLabel = healthError ? t('status.refreshFailed', { error: healthError }) : t('status.refresh')

  return (
    <div className="ark-shell">
      <div
        className="ark-cols"
        style={{ '--ark-left': `${leftWidth}px`, '--ark-right': `${rightWidth}px` } as React.CSSProperties}
      >
        {leftCollapsed ? (
          <button
            type="button"
            className="ark-sidebar-tab"
            aria-label={t('workbench.layout.showThreads')}
            title={t('workbench.layout.showThreads')}
            onClick={toggleLeft}
          >
            ‹
          </button>
        ) : (
          <>
            <div className="ark-col ark-col--left">
              {left}
              <footer className="ark-sidebar__foot">
                <div
                  className="ark-sidebar__status"
                  role="status"
                  aria-live="polite"
                  aria-label={[onlineLabel, healthError].filter(Boolean).join(' ')}
                >
                  <span
                    className={`ark-dot ark-dot--${online === 'online' ? 'ok' : online === 'offline' ? 'bad' : 'idle'}`}
                    aria-hidden="true"
                  />
                  <span className="ark-sidebar__status-label">{onlineLabel}</span>
                  <button
                    type="button"
                    className="ark-sidebar__refresh"
                    title={refreshLabel}
                    aria-label={refreshLabel}
                    onClick={() => void refresh()}
                  >
                    ↻
                  </button>
                </div>
                <ViewSwitch />
              </footer>
            </div>
            <Resizer side="left" width={leftWidth} setWidth={setLeftWidth} />
          </>
        )}

        <div className="ark-col ark-col--center">
          <div className="ark-workbench-tools" aria-label={t('workbench.layout.aria')}>
            {leftCollapsed ? null : (
              <button
                type="button"
                className="ark-workbench-tools__btn"
                aria-pressed={!leftCollapsed}
                title={t('workbench.layout.hideThreads')}
                aria-label={t('workbench.layout.hideThreads')}
                onClick={toggleLeft}
              >
                ‹
              </button>
            )}
            <button
              type="button"
              className="ark-workbench-tools__btn"
              aria-pressed={!rightCollapsed}
              title={
                rightCollapsed ? t('workbench.layout.openContext') : t('workbench.layout.closeContext')
              }
              aria-label={
                rightCollapsed ? t('workbench.layout.openContext') : t('workbench.layout.closeContext')
              }
              onClick={toggleRight}
            >
              ›
            </button>
          </div>
          {center}
        </div>

        {rightCollapsed ? null : (
          <>
            <button
              type="button"
              className="ark-drawer-scrim"
              aria-label={t('workbench.layout.closeContext')}
              onClick={toggleRight}
            />
            <aside className="ark-drawer" style={{ '--ark-right': `${rightWidth}px` } as React.CSSProperties}>
              {right}
            </aside>
          </>
        )}
      </div>
    </div>
  )
}
