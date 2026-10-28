import type { ReactNode } from 'react'
import { Tabs } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import type { PanelId } from '../../code-session/state/workbench-slice'
import { ChangesPanel } from './ChangesPanel'
import { FilePanel } from './FilePanel'
import { PlanPanel } from './PlanPanel'
import { TerminalPanel } from './TerminalPanel'

const PANEL_TABS: { id: PanelId; label: string }[] = [
  { id: 'changes', label: '变更' },
  { id: 'plan', label: '计划' },
  { id: 'file', label: '文件' },
  { id: 'terminal', label: '终端' }
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
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  return (
    <section className="ark-panelslot" aria-label="任务上下文">
      <header className="ark-panelslot__head">
        <h2>任务上下文</h2>
        <button type="button" className="ark-panelslot__close" onClick={toggleRight} aria-label="关闭任务上下文" title="关闭任务上下文">
          ×
        </button>
      </header>
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
