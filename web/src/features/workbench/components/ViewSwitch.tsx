/**
 * Top-bar switch between the two app views.
 *
 * Two buttons and one store field: no router, no history coupling. Styling
 * comes from workbench.css, the primitive from shared/ui.
 */
import { useI18n } from '../../../shared/i18n'
import { APP_VIEWS, APP_VIEW_LABEL_KEYS, useViewStore, type AppView } from '../state/view-slice'

export function ViewSwitch(): JSX.Element {
  const { t } = useI18n()
  const view = useViewStore((state) => state.view)
  const setView = useViewStore((state) => state.setView)

  return (
    <div className="ark-viewswitch" role="group" aria-label={t('workbench.view.aria')}>
      {APP_VIEWS.map((id: AppView) => (
        <button
          key={id}
          type="button"
          className={`ark-viewswitch__btn${id === view ? ' is-active' : ''}`}
          aria-pressed={id === view}
          onClick={() => setView(id)}
        >
          {t(APP_VIEW_LABEL_KEYS[id])}
        </button>
      ))}
    </div>
  )
}
