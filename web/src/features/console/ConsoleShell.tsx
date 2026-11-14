/**
 * Console shell frame (approved proposal A).
 *
 * Owns the two global chrome regions — top bar and status bar — plus the ⌘K
 * palette. The stage is a slot: App decides which scene (workbench, review,
 * new task, settings) renders inside it, so the chrome never re-mounts when
 * the scene changes. The old activity rail is gone: its scene switches were
 * all reachable from context-local affordances and its only unique entry
 * (settings) moved to the top bar's right cluster.
 */
import { useEffect, type ReactNode } from 'react'
import { useViewStore } from '../workbench/state/view-slice'
import type { ConsoleMeta } from './hooks/use-console-meta'
import { CommandPalette } from './components/CommandPalette'
import { ConsoleStatusBar } from './components/ConsoleStatusBar'
import { ConsoleTopBar } from './components/ConsoleTopBar'
import './console.css'

export function ConsoleShell({ meta, children }: { meta: ConsoleMeta; children: ReactNode }): JSX.Element {
  const toggleCommand = useViewStore((s) => s.toggleCommand)
  const setCommandOpen = useViewStore((s) => s.setCommandOpen)

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        toggleCommand()
      } else if (event.key === 'Escape') {
        setCommandOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [toggleCommand, setCommandOpen])

  return (
    <div className="console-shell">
      <ConsoleTopBar meta={meta} />
      <div className="console-body">
        <div className="console-stage">
          <div className="console-stage__view">{children}</div>
        </div>
      </div>
      <ConsoleStatusBar meta={meta} />
      <CommandPalette />
    </div>
  )
}
