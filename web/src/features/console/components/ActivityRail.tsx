/**
 * Activity rail: the console's primary navigation.
 *
 * Workbench / review / new task are views; terminal and plan open the resident
 * inspector on the matching tab. Nothing here fetches — it only moves layout
 * state, so a rail click can never start a runtime request.
 */
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'
import type { PanelId } from '../../code-session/state/workbench-slice'

type RailItem = {
  id: string
  icon: string
  labelKey: Parameters<ReturnType<typeof useI18n>['t']>[0]
  active: boolean
  run: () => void
}

export function ActivityRail(): JSX.Element {
  const { t } = useI18n()
  const view = useViewStore((s) => s.view)
  const setView = useViewStore((s) => s.setView)
  const activePanel = useCodeSessionStore((s) => s.activePanel)
  const openPanel = useCodeSessionStore((s) => s.openPanel)

  // A rail click is an explicit "show me this context view" intent, so it is
  // allowed to expand the inspector the user may have collapsed.
  const showPanel = (panel: PanelId): void => {
    openPanel(panel)
    setView('code')
  }

  const items: RailItem[] = [
    {
      id: 'workbench',
      icon: 'chat',
      labelKey: 'console.rail.workbench',
      active: view === 'code' && activePanel !== 'terminal' && activePanel !== 'plan',
      run: () => setView('code')
    },
    {
      id: 'review',
      icon: 'diff',
      labelKey: 'console.rail.review',
      active: view === 'review',
      run: () => setView('review')
    },
    {
      id: 'new',
      icon: 'plus',
      labelKey: 'console.rail.new',
      active: view === 'new',
      run: () => setView('new')
    },
    {
      id: 'terminal',
      icon: 'terminal',
      labelKey: 'console.rail.terminal',
      active: view === 'code' && activePanel === 'terminal',
      run: () => showPanel('terminal')
    },
    {
      id: 'plan',
      icon: 'clock',
      labelKey: 'console.rail.plan',
      active: view === 'code' && activePanel === 'plan',
      run: () => showPanel('plan')
    }
  ]

  return (
    <nav className="console-rail" aria-label={t('console.rail.aria')}>
      <span className="console-mark console-mark--sm" aria-hidden="true">A</span>

      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          className={`console-rail__btn${item.active ? ' is-active' : ''}`}
          onClick={item.run}
          aria-label={t(item.labelKey)}
          aria-current={item.active ? 'page' : undefined}
          title={t(item.labelKey)}
        >
          <Icon name={item.icon} size={17} />
        </button>
      ))}

      <span className="console-rail__grow" />
      <span className="console-rail__sep" aria-hidden="true" />

      <button
        type="button"
        className={`console-rail__btn${view === 'settings' ? ' is-active' : ''}`}
        onClick={() => setView('settings')}
        aria-label={t('console.rail.settings')}
        aria-current={view === 'settings' ? 'page' : undefined}
        title={t('console.rail.settings')}
      >
        <Icon name="settings" size={17} />
      </button>
    </nav>
  )
}