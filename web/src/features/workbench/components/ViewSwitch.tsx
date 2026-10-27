/**
 * Top-bar switch between the two app views.
 *
 * Two buttons and one store field: no router, no history coupling. Styling
 * comes from workbench.css, the primitive from shared/ui.
 */
import { APP_VIEWS, APP_VIEW_LABELS, useViewStore, type AppView } from '../state/view-slice'

export function ViewSwitch(): JSX.Element {
  const view = useViewStore((state) => state.view)
  const setView = useViewStore((state) => state.setView)

  return (
    <div className="ark-viewswitch" role="group" aria-label="视图切换">
      {APP_VIEWS.map((id: AppView) => (
        <button
          key={id}
          type="button"
          className={`ark-viewswitch__btn${id === view ? ' is-active' : ''}`}
          aria-pressed={id === view}
          onClick={() => setView(id)}
        >
          {APP_VIEW_LABELS[id]}
        </button>
      ))}
    </div>
  )
}
