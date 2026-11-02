import { useEffect, useRef, type KeyboardEvent, type ReactNode } from 'react'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useRuntimeStatusLabels } from '../../code-session/hooks/use-runtime-status-labels'
import { ViewSwitch } from '../components/ViewSwitch'
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
 * Conversation-first workbench, no global top bar: the thread rail is a
 * full-height sidebar whose footer carries the runtime status and the view
 * switch, and the context projection is a floating drawer over the canvas.
 * The drawer stays mounted while hidden (`hidden` attribute, not conditional
 * rendering) so panel drafts and server read models survive open/close.
 */
export function WorkbenchLayout({ left, center, right }: WorkbenchLayoutProps): JSX.Element {
  const { t } = useI18n()
  const leftWidth = useCodeSessionStore((s) => s.leftWidth)
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const leftCollapsed = useCodeSessionStore((s) => s.leftCollapsed)
  const setLeftWidth = useCodeSessionStore((s) => s.setLeftWidth)
  const toggleLeft = useCodeSessionStore((s) => s.toggleLeft)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  // The sidebar footer owns liveness: the same label mapping the settings top
  // bar uses, rendered as one compact row instead of a full-width banner.
  const { online, onlineLabel, refreshLabel, healthError, refresh } = useRuntimeStatusLabels()

  // Drawer semantics: focus moves into the dialog on open and back to the
  // toggle on close; Escape closes. The drawer keeps its panel state while
  // hidden, so only focus management is event-driven here.
  const drawerRef = useRef<HTMLElement | null>(null)
  const drawerToggleRef = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    if (rightCollapsed) {
      drawerToggleRef.current?.focus()
      return
    }
    drawerRef.current?.focus()
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.key === 'Escape') toggleRight()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [rightCollapsed, toggleRight])

  return (
    <div className="ark-shell">
      <div
        className="ark-cols"
        style={{ '--ark-left': `${leftWidth}px` } as React.CSSProperties}
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
                <div className="ark-sidebar__foot-row">
                  <div
                    className="ark-sidebar__status"
                    role="status"
                    aria-live="polite"
                    aria-label={[onlineLabel, healthError]
                      .filter(Boolean)
                      .join(t('status.ariaSeparator'))}
                  >
                    <span
                      className={`ark-dot ark-dot--${online === 'online' ? 'ok' : online === 'offline' ? 'bad' : 'idle'}`}
                      aria-hidden="true"
                    />
                    <span className="ark-sidebar__status-label">{onlineLabel}</span>
                  </div>
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
            <Resizer width={leftWidth} setWidth={setLeftWidth} />
          </>
        )}

        <div className="ark-col ark-col--center">
          <div className="ark-workbench-tools" aria-label={t('workbench.layout.aria')}>
            {leftCollapsed ? null : (
              <button
                type="button"
                className="ark-workbench-tools__btn"
                title={t('workbench.layout.hideThreads')}
                aria-label={t('workbench.layout.hideThreads')}
                onClick={toggleLeft}
              >
                ‹
              </button>
            )}
            <button
              ref={drawerToggleRef}
              type="button"
              className="ark-workbench-tools__btn"
              aria-expanded={!rightCollapsed}
              aria-controls="ark-context-drawer"
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

        <button
          type="button"
          className="ark-drawer-scrim"
          hidden={rightCollapsed}
          aria-label={t('workbench.layout.closeContext')}
          onClick={toggleRight}
        />
        <aside
          ref={drawerRef}
          id="ark-context-drawer"
          className="ark-drawer"
          hidden={rightCollapsed}
          role="dialog"
          aria-modal="true"
          aria-label={t('workbench.layout.context')}
          tabIndex={-1}
        >
          {right}
        </aside>
      </div>
    </div>
  )
}
