/**
 * 「定时任务」 entry on the thread rail: create one task without leaving Code.
 *
 * The form, its validation and the wire mapping are the settings page's own
 * (`schedule-form.ts` + `ScheduleForm`), and the write goes through the same
 * `useSchedules().create`, so this dialog adds an entry point and nothing else.
 * The runtime stays the authority on cadence normalization and the task cap.
 *
 * A created task is only ever re-found in the settings page's task list, so the
 * header links there instead of leaving creation a dead end; the jump is the
 * same `setView('settings', section)` request the console shell already uses.
 */
import { useEffect, useState } from 'react'
import { useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useViewStore } from '../../workbench/state/view-slice'
import { useSchedules } from '../../settings/hooks/use-schedules'
import { useProviders } from '../../settings/hooks/use-providers'
import { toModelProviderGroups } from '../../../shared/model-provider-menu'
import { describeOutcome } from '../../settings/api/outcome'
import { ScheduleForm } from '../../settings/components/ScheduleForm'
import {
  buildCreateRequest,
  emptyScheduleDraft,
  type ScheduleDraft
} from '../../settings/components/schedule-form'

export function ScheduleCreateDialog({ onClose }: { onClose: () => void }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const api = useSchedules()
  const providers = useProviders()
  const groups = toModelProviderGroups(providers.providers)
  const active = providers.providers.find((row) => row.id === (providers.activeProviderId ?? '')) ??
    providers.providers.find((row) => row.is_default) ?? null
  const fallback = active ? { providerId: active.id, model: active.model } : undefined
  const [draft, setDraft] = useState<ScheduleDraft>(() => emptyScheduleDraft())
  const [formError, setFormError] = useState<string | null>(null)
  const setView = useViewStore((state) => state.setView)

  // Escape dismisses the dialog, matching the workbench's context drawer.
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const submit = async (): Promise<void> => {
    setFormError(null)
    const built = buildCreateRequest(draft)
    if (!built.ok) {
      setFormError(built.error)
      return
    }
    const outcome = await api.create(built.request)
    if (!outcome.ok) {
      setFormError(describeOutcome(outcome))
      toast.push('error', t('settings.schedules.saveFailed'), describeOutcome(outcome))
      return
    }
    toast.push('success', t('settings.schedules.created', { title: outcome.value.title }))
    onClose()
  }

  return (
    <div
      className="ark-scheduledialog"
      role="dialog"
      aria-modal="true"
      aria-label={t('settings.schedules.createTitle')}
    >
      <button
        type="button"
        className="ark-scheduledialog__scrim"
        aria-label={t('common.close')}
        onClick={onClose}
      />
      <div className="ark-scheduledialog__panel">
        <header className="ark-scheduledialog__head">
          <strong>{t('settings.schedules.createTitle')}</strong>
          <div className="ark-scheduledialog__head-actions">
            <button
              type="button"
              className="ark-linkbtn"
              onClick={() => {
                onClose()
                setView('settings', 'schedules')
              }}
            >
              {t('settings.schedules.openAll')}
            </button>
            <button
              type="button"
              className="ark-scheduledialog__close"
              aria-label={t('common.close')}
              title={t('common.close')}
              onClick={onClose}
            >
              ×
            </button>
          </div>
        </header>
        <div className="ark-scheduledialog__body">
          <ScheduleForm
            draft={draft}
            editing={false}
            busy={api.busy !== null}
            error={formError}
            unchanged={false}
            groups={groups}
            fallback={fallback}
            onChange={setDraft}
            onSubmit={() => void submit()}
            onCancel={onClose}
          />
        </div>
      </div>
    </div>
  )
}
