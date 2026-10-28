import type { ReactNode } from 'react'
import { Tabs } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import type { PanelId } from '../../code-session/state/workbench-slice'
import { ChangesPanel } from './ChangesPanel'
import { FilePanel } from './FilePanel'
import { PlanPanel } from './PlanPanel'
import { TerminalPanel } from './TerminalPanel'

const PANEL_TABS: { id: PanelId; label: string }[] = [
  { id: 'changes', label: 'Changes' },
  { id: 'plan', label: 'Plan' },
  { id: 'file', label: 'File' },
  { id: 'terminal', label: 'Terminal' }
]

function PanelBody({ panel }: { panel: PanelId }): ReactNode {
  switch (panel) {
    case 'changes':
      return <ChangesPanel />
    case 'plan':
      return <PlanPanel />
    case 'file':
      return <FilePanel />
    case 'terminal':
      return <TerminalPanel />
    default:
      return null
  }
}

/**
 * The right-hand slot: four registered panels behind one tab strip.
 *
 * Switching panels is layout state (`workbench` slice), not thread state, so a
 * panel never triggers a runtime request.
 */
export function PanelSlot(): JSX.Element {
  const activePanel = useCodeSessionStore((s) => s.activePanel)
  const setActivePanel = useCodeSessionStore((s) => s.setActivePanel)

  return (
    <section className="ark-panelslot">
      <Tabs
        items={PANEL_TABS.map((tab) => ({ id: tab.id, label: tab.label }))}
        active={activePanel}
        onChange={setActivePanel}
        label="工作台面板"
      />
      <div className="ark-panelslot__body">
        <PanelBody panel={activePanel} />
      </div>
    </section>
  )
}
