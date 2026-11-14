/**
 * Activity rail: the console's only global navigation.
 *
 * Every scene switch the old rail offered is reachable from context-local
 * affordances: new thread from the thread list, review from the thread header
 * and the review page, inspector panels from the workbench toggles. A rail
 * click must never start a runtime request, so it only moves layout state.
 */
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useViewStore } from '../../workbench/state/view-slice'

export function ActivityRail(): JSX.Element {
  const { t } = useI18n()
  const view = useViewStore((s) => s.view)
  const setView = useViewStore((s) => s.setView)

  return (
    <nav className="console-rail" aria-label={t('console.rail.aria')}>
      <span className="console-mark console-mark--sm" aria-hidden="true">A</span>

      <span className="console-rail__grow" />

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
