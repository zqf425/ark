import type { ReactNode } from 'react'
import { Tabs } from '../../../shared/ui'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import type { PanelId } from '../../code-session/state/workbench-slice'
import { ChangesPanel } from './ChangesPanel'
import { FilePanel } from './FilePanel'
import { PlanPanel } from './PlanPanel'
import { TerminalPanel } from './TerminalPanel'

const PANEL_TABS: { id: PanelId; labelKey: MessageKey }[] = [
  { id: 'changes', labelKey: 'workbench.panel.changes' },
  { id: 'plan', labelKey: 'workbench.panel.plan' },
  { id: 'file', labelKey: 'workbench.panel.file' },
  { id: 'terminal', labelKey: 'workbench.panel.terminal' }
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
  const { t } = useI18n()
  const activePanel = useCodeSessionStore((s) => s.activePanel)
  const setActivePanel = useCodeSessionStore((s) => s.setActivePanel)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  return (
    <section className="ark-panelslot" aria-label={t('workbench.slot.title')}>
      <header className="ark-panelslot__head">
        <h2>{t('workbench.slot.title')}</h2>
        <button
          type="button"
          className="ark-panelslot__close"
          onClick={toggleRight}
          aria-label={t('workbench.slot.close')}
          title={t('workbench.slot.close')}
        >
          ×
        </button>
      </header>
      <Tabs
        items={PANEL_TABS.map((tab) => ({ id: tab.id, label: t(tab.labelKey) }))}
        active={activePanel}
        onChange={setActivePanel}
        label={t('workbench.slot.tabsAria')}
      />
      <div className="ark-panelslot__body">
        <PanelBody panel={activePanel} />
      </div>
    </section>
  )
}
