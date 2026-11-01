/**
 * Schedule create/edit form: fields only.
 *
 * State and validation live in `schedule-form.ts`, submission in
 * `ScheduleSection`. This file decides which control a cadence needs, so a user
 * switching between daily and weekly sees exactly the fields that matter and
 * the other cadence values are preserved untouched.
 */
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ScheduleKind } from '../../../shared/contracts'
import { CheckField, Field, SelectField, TextField } from './Field'
import { Notice } from './SectionShell'
import {
  MONTH_DAYS,
  SCHEDULE_AUTONOMY_VALUES,
  SCHEDULE_KINDS,
  SCHEDULE_KIND_LABEL_KEYS,
  WEEKDAYS,
  weekdayLabelKey
} from './schedule-cadence'
import { MAX_EVERY_MINUTES, MIN_EVERY_MINUTES, type ScheduleDraft } from './schedule-form'

export type ScheduleFormProps = {
  draft: ScheduleDraft
  editing: boolean
  busy: boolean
  error: string | null
  /** True when saving would send nothing (edit mode with no change). */
  unchanged: boolean
  onChange: (draft: ScheduleDraft) => void
  onSubmit: () => void
  onCancel: () => void
}

function toggleDay(days: number[], day: number): number[] {
  return days.includes(day) ? days.filter((value) => value !== day) : [...days, day]
}

export function ScheduleForm({
  draft,
  editing,
  busy,
  error,
  unchanged,
  onChange,
  onSubmit,
  onCancel
}: ScheduleFormProps): JSX.Element {
  const { t } = useI18n()
  const set = (patch: Partial<ScheduleDraft>): void => onChange({ ...draft, ...patch })
  const needsClock = draft.kind === 'daily' || draft.kind === 'weekly' || draft.kind === 'monthly'

  return (
    <div className="ark-card ark-card--editing">
      <div className="ark-card__meta">
        <strong>{editing ? t('settings.schedules.editTitle', { title: draft.title || draft.prompt }) : t('settings.schedules.createTitle')}</strong>
      </div>

      <div className="ark-settings__grid">
        <TextField
          id="schedule-title"
          label={t('settings.schedules.field.title')}
          hint={t('settings.schedules.field.titleHint')}
          value={draft.title}
          disabled={busy}
          onChange={(value) => set({ title: value })}
        />
        <Field label={t('settings.schedules.field.prompt')} htmlFor="schedule-prompt" hint={t('settings.schedules.field.promptHint')}>
          <textarea
            id="schedule-prompt"
            className="ark-field__area"
            value={draft.prompt}
            disabled={busy}
            spellCheck={false}
            onChange={(event) => set({ prompt: event.target.value })}
          />
        </Field>
      </div>

      <div className="ark-settings__grid">
        <SelectField<ScheduleKind>
          id="schedule-kind"
          label={t('settings.schedules.field.cadence')}
          value={draft.kind}
          options={SCHEDULE_KINDS}
          labels={Object.fromEntries(
            SCHEDULE_KINDS.map((kind) => [kind, t(SCHEDULE_KIND_LABEL_KEYS[kind])])
          ) as Record<ScheduleKind, string>}
          disabled={busy}
          onChange={(value) => set({ kind: value })}
        />

        {draft.kind === 'interval' ? (
          <TextField
            id="schedule-every-minutes"
            type="number"
            label={t('settings.schedules.field.everyMinutes')}
            hint={t('settings.schedules.field.everyMinutesHint', { min: MIN_EVERY_MINUTES, max: MAX_EVERY_MINUTES })}
            value={draft.everyMinutes}
            disabled={busy}
            onChange={(value) => set({ everyMinutes: value })}
          />
        ) : null}

        {draft.kind === 'at' ? (
          <Field label={t('settings.schedules.field.atTime')} htmlFor="schedule-at-time">
            <input
              id="schedule-at-time"
              className="ark-field__input"
              type="datetime-local"
              value={draft.atTime}
              disabled={busy}
              onChange={(event) => set({ atTime: event.target.value })}
            />
          </Field>
        ) : null}

        {needsClock ? (
          <TextField
            id="schedule-time-of-day"
            label={t('settings.schedules.field.timeOfDay')}
            value={draft.timeOfDay}
            disabled={busy}
            placeholder="09:00"
            onChange={(value) => set({ timeOfDay: value })}
          />
        ) : null}
      </div>

      {draft.kind === 'weekly' ? (
        <Field label={t('settings.schedules.field.weekdays')} htmlFor="schedule-weekdays">
          <div className="ark-settings__row" id="schedule-weekdays">
            {WEEKDAYS.map((day) => (
              <CheckField
                key={day}
                label={t(weekdayLabelKey(day))}
                checked={draft.dayOfWeek.includes(day)}
                disabled={busy}
                onChange={() => set({ dayOfWeek: toggleDay(draft.dayOfWeek, day) })}
              />
            ))}
          </div>
        </Field>
      ) : null}

      {draft.kind === 'monthly' ? (
        <Field label={t('settings.schedules.field.monthDays')} htmlFor="schedule-month-days">
          <div className="ark-settings__row" id="schedule-month-days">
            {MONTH_DAYS.map((day) => (
              <CheckField
                key={day}
                label={String(day)}
                checked={draft.dayOfMonth.includes(day)}
                disabled={busy}
                onChange={() => set({ dayOfMonth: toggleDay(draft.dayOfMonth, day) })}
              />
            ))}
          </div>
        </Field>
      ) : null}

      <div className="ark-settings__grid">
        <SelectField<string>
          id="schedule-autonomy"
          label={t('settings.schedules.field.autonomy')}
          hint={t('settings.schedules.field.autonomyHint')}
          value={draft.autonomy}
          options={SCHEDULE_AUTONOMY_VALUES}
          labels={{
            strict: t('session.runtime.strict'),
            workspace_write: t('session.runtime.workspaceWrite'),
            unattended: t('session.runtime.unattended')
          }}
          disabled={busy}
          onChange={(value) => set({ autonomy: value })}
        />
        <TextField
          id="schedule-model"
          label={t('settings.schedules.field.model')}
          hint={t('settings.schedules.field.modelHint')}
          value={draft.model}
          disabled={busy}
          onChange={(value) => set({ model: value })}
        />
        <TextField
          id="schedule-provider"
          label={t('settings.schedules.field.provider')}
          hint={t('settings.schedules.field.providerHint')}
          value={draft.providerId}
          disabled={busy}
          mono
          onChange={(value) => set({ providerId: value })}
        />
        <TextField
          id="schedule-workspace"
          label={t('settings.schedules.field.workspace')}
          hint={t('settings.schedules.field.workspaceHint')}
          value={draft.workspace}
          disabled={busy}
          mono
          onChange={(value) => set({ workspace: value })}
        />
      </div>

      {error ? <Notice tone="error" title={t('settings.schedules.saveFailed')}>{error}</Notice> : null}

      <div className="ark-settings__actions">
        <Button variant="primary" size="sm" disabled={busy || unchanged} onClick={onSubmit}>
          {busy ? t('common.saving') : t('common.save')}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </div>
  )
}
