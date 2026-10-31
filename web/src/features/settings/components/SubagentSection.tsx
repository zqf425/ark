/**
 * Subagent section: enabled, concurrency, and explicit auto-continue opt-in.
 *
 * The runtime hard-limits concurrency to 1–8; the input is clamped here too so
 * the user sees the limit instead of a rejected save. A `rejected` answer is
 * rendered verbatim — `restart_required` is never swallowed.
 */
import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { SUBAGENT_MAX_CONCURRENCY, SUBAGENT_MIN_CONCURRENCY, type ApplyResult, type SubagentSettings } from '../../../shared/contracts'
import type { SettingsApi } from '../hooks/use-settings'
import { describeOutcome, type Outcome } from '../api/outcome'
import { CheckField, Field } from './Field'
import { Notice, SettingsSection, Skeleton } from './SectionShell'

function clampConcurrency(value: number): number {
  if (!Number.isFinite(value)) return SUBAGENT_MIN_CONCURRENCY
  return Math.min(SUBAGENT_MAX_CONCURRENCY, Math.max(SUBAGENT_MIN_CONCURRENCY, Math.round(value)))
}

export function SubagentSection({ settings }: { settings: SettingsApi }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const { config, saving, save } = settings
  const [enabled, setEnabled] = useState(false)
  const [autoContinue, setAutoContinue] = useState(false)
  const [concurrency, setConcurrency] = useState('2')
  const [rejection, setRejection] = useState<{ reason: string; restartRequired: string[] } | null>(null)

  // Adopt runtime values whenever the loaded config changes (first load and
  // after each successful save), but never fight an in-flight edit otherwise.
  useEffect(() => {
    if (!config) return
    setEnabled(config.subagents.enabled)
    setAutoContinue(config.subagents.auto_continue)
    setConcurrency(String(config.subagents.max_concurrency))
  }, [config])

  if (!config) {
    return (
      <SettingsSection title={t('settings.subagents.title')} desc={t('settings.subagents.descLoading')}>
        <Skeleton rows={2} />
      </SettingsSection>
    )
  }

  const parsed = Number(concurrency)
  const invalid = !Number.isInteger(parsed) || parsed < SUBAGENT_MIN_CONCURRENCY || parsed > SUBAGENT_MAX_CONCURRENCY
  const busy = saving === 'subagents'
  const dirty =
    enabled !== config.subagents.enabled ||
    autoContinue !== config.subagents.auto_continue ||
    parsed !== config.subagents.max_concurrency ||
    invalid

  const persist = async (payload: SubagentSettings, label: string): Promise<void> => {
    setRejection(null)
    const outcome: Outcome<ApplyResult> = await save({ subagents: payload }, 'subagents')
    if (!outcome.ok) {
      toast.push('error', t('settings.subagents.saveFailed', { label }), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      // Show the runtime's own words, both fields, without truncation.
      setRejection({ reason: outcome.value.reason, restartRequired: outcome.value.restart_required })
      toast.push('error', t('settings.subagents.rejected'), outcome.value.reason)
      return
    }
    toast.push(
      'success',
      t('settings.subagents.saveSucceeded', { label }),
      t('settings.subagents.changed', {
        keys: outcome.value.changed.join(t('common.listSeparator')) || t('common.none')
      })
    )
  }

  const toggleEnabled = async (next: boolean): Promise<void> => {
    setEnabled(next)
    await persist(
      {
        enabled: next,
        max_concurrency: clampConcurrency(Number(concurrency)),
        auto_continue: autoContinue,
      },
      next ? t('settings.subagents.startLabel') : t('settings.subagents.stopLabel'),
    )
  }

  const toggleAutoContinue = async (next: boolean): Promise<void> => {
    setAutoContinue(next)
    await persist(
      {
        enabled,
        max_concurrency: clampConcurrency(Number(concurrency)),
        auto_continue: next,
      },
      next ? t('settings.subagents.autoContinue.enabled') : t('settings.subagents.autoContinue.disabled'),
    )
  }

  return (
    <SettingsSection title={t('settings.subagents.title')} desc={t('settings.subagents.description')}>
      <div className="ark-card__foot">
        <CheckField
          label={t('settings.subagents.enabledLabel')}
          checked={enabled}
          disabled={busy}
          onChange={(next) => void toggleEnabled(next)}
        />
        <span className="ark-settings__hint">{t('settings.subagents.toggleHint')}</span>
      </div>

      <div className="ark-card__foot">
        <CheckField
          label={t('settings.subagents.autoContinue.label')}
          checked={autoContinue}
          disabled={busy || !enabled}
          onChange={(next) => void toggleAutoContinue(next)}
        />
        <span className="ark-settings__hint">{t('settings.subagents.autoContinue.hint')}</span>
      </div>

      <Field
        label={t('settings.subagents.concurrency.label')}
        htmlFor="subagent-concurrency"
        hint={t('settings.subagents.concurrency.hint', { min: SUBAGENT_MIN_CONCURRENCY, max: SUBAGENT_MAX_CONCURRENCY })}
      >
        <div className="ark-field__inline">
          <input
            id="subagent-concurrency"
            className="ark-field__input"
            type="number"
            min={SUBAGENT_MIN_CONCURRENCY}
            max={SUBAGENT_MAX_CONCURRENCY}
            step={1}
            value={concurrency}
            disabled={busy}
            onChange={(event) => setConcurrency(event.target.value)}
            onBlur={() => {
              if (invalid) setConcurrency(String(config.subagents.max_concurrency))
            }}
          />
          <Button
            variant="primary"
            size="sm"
            disabled={busy || invalid || !dirty}
            onClick={() =>
              void persist(
                {
                  enabled,
                  max_concurrency: clampConcurrency(parsed),
                  auto_continue: autoContinue,
                },
                t('settings.subagents.saveConcurrency'),
              )
            }
          >
            {busy ? t('common.saving') : t('common.save')}
          </Button>
        </div>
      </Field>

      {invalid ? (
        <p className="ark-settings__hint ark-settings__hint--error">
          {t('settings.subagents.concurrency.invalid', { min: SUBAGENT_MIN_CONCURRENCY, max: SUBAGENT_MAX_CONCURRENCY })}
        </p>
      ) : null}

      {rejection ? (
        <Notice tone="error" title={t('settings.subagents.rejectedTitle')}>
          <span>{t('settings.subagents.reason', { reason: rejection.reason })}</span>
          {rejection.restartRequired.length > 0 ? (
            <span>
              {t('settings.subagents.restartRequired', {
                keys: rejection.restartRequired.join(t('common.listSeparator'))
              })}
            </span>
          ) : (
            <span>{t('settings.subagents.restartNone')}</span>
          )}
        </Notice>
      ) : null}

      <p className="ark-settings__hint">
        {t('settings.subagents.autoContinue.runtime')}: {String(config.subagents.auto_continue)}
      </p>
    </SettingsSection>
  )
}
