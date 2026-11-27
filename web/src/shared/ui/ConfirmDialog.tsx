/**
 * The app's shared confirm dialog for destructive gates (delete project /
 * thread on the Code rail, plus future settings sections). Native
 * `window.confirm` is not a usable gate in the desktop WebView: Tauri/WKWebView
 * swallows it silently, so destructive verbs ran through with no confirmation
 * at all. Rendering our own dialog keeps the callers' "await-then-act" shape
 * but with a surface that always renders and is testable in jsdom.
 *
 * Focus/Escape/Tab-trap come from `shared/ui/focus.ts` (`useDialogFocus`), the
 * same contract the provider delete dialog uses; no extra document-level
 * Escape listener may be stacked on top (design §7.10.2). Initial focus lands
 * on the cancel button so a bare Enter never confirms a destructive action.
 */
import { Button } from './Button'
import { useDialogFocus } from './focus'
import './confirm.css'

export type ConfirmDialogProps = {
  /** Shown as the dialog heading. */
  title: string
  /** Danger-styled confirm button label (e.g. "删除"). */
  confirmLabel: string
  /** Cancel button label; also the scrim's accessible name. */
  cancelLabel: string
  /** Optional longer body under the title. */
  detail?: string
  /** Called with the user's choice; the dialog unmounts afterwards. */
  onResolve: (confirmed: boolean) => void
}

/** Modal confirm surface where every dismissal path but the confirm button cancels. */
export function ConfirmDialog({ title, confirmLabel, cancelLabel, detail, onResolve }: ConfirmDialogProps): JSX.Element {
  const cancel = (): void => onResolve(false)
  const dialog = useDialogFocus(true, cancel)
  return (
    <div className="ark-confirmdialog">
      <button type="button" className="ark-confirmdialog__scrim" tabIndex={-1} aria-label={cancelLabel} onClick={cancel} />
      <div ref={dialog.ref} tabIndex={-1} onKeyDown={dialog.onKeyDown} className="ark-confirmdialog__panel" role="dialog" aria-modal="true" aria-label={title}>
        <strong className="ark-confirmdialog__title">{title}</strong>
        {detail ? <p className="ark-confirmdialog__detail">{detail}</p> : null}
        <div className="ark-confirmdialog__actions">
          <Button data-autofocus variant="ghost" size="sm" onClick={cancel}>{cancelLabel}</Button>
          <Button variant="danger" size="sm" onClick={() => onResolve(true)}>{confirmLabel}</Button>
        </div>
      </div>
    </div>
  )
}
