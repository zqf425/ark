/**
 * App composition for the approved ARK Console shell (proposal A).
 *
 * The shell owns the top bar, activity rail and status bar; this file only
 * picks the scene that renders in the stage and keeps the one thread-stream
 * hook mounted across scene changes. No fetch, no event interpretation, no
 * styling decisions beyond choosing a scene.
 */
import { useEffect, useRef } from 'react'
import { ErrorBoundary } from './shared/ui'
import { useI18n } from './shared/i18n'
import { useCodeSessionStore } from './features/code-session/state/store'
import { useThreadList } from './features/code-session/hooks/use-thread-list'
import { useThreadStream } from './features/code-session/hooks/use-thread-stream'
import { ThreadListPanel } from './features/code-session/components/ThreadListPanel'
import { ThreadStream } from './features/code-session/components/ThreadStream'
import { PanelSlot } from './features/workbench/panels/PanelSlot'
import { WorkbenchLayout } from './features/workbench/layout/WorkbenchLayout'
import { useViewStore } from './features/workbench/state/view-slice'
import { SettingsPage } from './features/settings/components/SettingsPage'
import { ConsoleShell } from './features/console/ConsoleShell'
import { NewTaskPage } from './features/console/components/NewTaskPage'
import { ReviewPage } from './features/console/components/ReviewPage'
import { useConsoleMeta } from './features/console/hooks/use-console-meta'

/**
 * Delegation becomes visible: the first live subagent opens the inspector on
 * its tab (explicit `openPanel` intent, like selecting a file). Later activity
 * only bumps the badge — the open/collapsed choice stays with the user, and a
 * count that returns to zero never closes anything.
 */
function useSubagentAutoReveal(): void {
  const activeCount = useCodeSessionStore((s) => s.childRunActiveCount)
  const openPanel = useCodeSessionStore((s) => s.openPanel)
  const rightCollapsed = useCodeSessionStore((s) => s.rightCollapsed)
  const everActive = useRef(false)

  useEffect(() => {
    if (activeCount > 0) {
      if (!everActive.current && rightCollapsed) {
        everActive.current = true
        openPanel('subagents')
      }
    } else {
      everActive.current = false
    }
  }, [activeCount, rightCollapsed, openPanel])
}

export function App(): JSX.Element {
  const { t } = useI18n()
  const view = useViewStore((state) => state.view)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const { refresh, create } = useThreadList()
  const meta = useConsoleMeta()
  useSubagentAutoReveal()

  // One bootstrap fetch. The stream hook below owns everything after that.
  useEffect(() => {
    void refresh()
  }, [refresh])

  useThreadStream(activeThreadId)

  return (
    <ConsoleShell meta={meta}>
      {view === 'settings' ? (
        <ErrorBoundary label={t('workbench.boundary.settings')}>
          <SettingsPage />
        </ErrorBoundary>
      ) : view === 'review' ? (
        <ErrorBoundary label={t('console.review.title')}>
          <ReviewPage />
        </ErrorBoundary>
      ) : view === 'new' ? (
        <ErrorBoundary label={t('console.new.title')}>
          <NewTaskPage meta={meta} />
        </ErrorBoundary>
      ) : (
        <WorkbenchLayout
          left={
            <ErrorBoundary label={t('workbench.boundary.threadList')}>
              <ThreadListPanel />
            </ErrorBoundary>
          }
          center={
            <ErrorBoundary label={t('workbench.boundary.stream')}>
              <ThreadStream threadId={activeThreadId} onCreateThread={create} />
            </ErrorBoundary>
          }
          right={
            <ErrorBoundary label={t('workbench.boundary.panel')}>
              <PanelSlot />
            </ErrorBoundary>
          }
        />
      )}
    </ConsoleShell>
  )
}
