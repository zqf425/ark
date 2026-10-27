/**
 * App composition (design §7.10.2).
 *
 * Two views share one top status bar: the Code workbench (default) and the
 * settings page. This file only wires panels to hooks and picks a view: no
 * fetch, no event interpretation, no styling decisions beyond layout slots.
 */
import { useEffect } from 'react'
import { ErrorBoundary } from './shared/ui'
import { useCodeSessionStore } from './features/code-session/state/store'
import { useThreadList } from './features/code-session/hooks/use-thread-list'
import { useThreadStream } from './features/code-session/hooks/use-thread-stream'
import { ThreadListPanel } from './features/code-session/components/ThreadListPanel'
import { ThreadStream } from './features/code-session/components/ThreadStream'
import { RuntimeStatusBar } from './features/code-session/components/RuntimeStatusBar'
import { PanelSlot } from './features/workbench/panels/PanelSlot'
import { WorkbenchLayout } from './features/workbench/layout/WorkbenchLayout'
import { useViewStore } from './features/workbench/state/view-slice'
import { SettingsPage } from './features/settings/components/SettingsPage'

export function App(): JSX.Element {
  const view = useViewStore((state) => state.view)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const { refresh } = useThreadList()

  // One bootstrap fetch. The stream hook below owns everything after that.
  useEffect(() => {
    void refresh()
  }, [refresh])

  useThreadStream(activeThreadId)

  if (view === 'settings') {
    return (
      <div className="ark-shell">
        <RuntimeStatusBar />
        <ErrorBoundary label="设置页崩溃">
          <SettingsPage />
        </ErrorBoundary>
      </div>
    )
  }

  return (
    <WorkbenchLayout
      top={<RuntimeStatusBar />}
      left={
        <ErrorBoundary label="会话列表崩溃">
          <ThreadListPanel />
        </ErrorBoundary>
      }
      center={
        <ErrorBoundary label="会话流崩溃">
          <ThreadStream threadId={activeThreadId} />
        </ErrorBoundary>
      }
      right={
        <ErrorBoundary label="面板崩溃">
          <PanelSlot />
        </ErrorBoundary>
      }
    />
  )
}
