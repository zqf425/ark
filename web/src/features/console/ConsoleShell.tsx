/**
 * Console shell frame (approved proposal A).
 *
 * Owns the three global chrome regions — top bar, activity rail and status
 * bar — plus the ⌘K palette. The stage is a slot: App decides which scene
 * (workbench, review, new task, settings) renders inside it, so the chrome
 * never re-mounts when the scene changes.
 */
import { useEffect, type ReactNode } from 'react'
import { useViewStore } from '../workbench/state/view-slice'
import type { ConsoleMeta } from './hooks/use-console-meta'
import { ActivityRail } from './components/ActivityRail'
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
        <ActivityRail />
        <div className="console-stage">
          <div className="console-stage__view">{children}</div>
        </div>
      </div>
      <ConsoleStatusBar meta={meta} />
      <CommandPalette />
    </div>
  )
}
