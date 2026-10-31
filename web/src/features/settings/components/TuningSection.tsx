/**
 * Hot turn and SSE limits.
 *
 * Values are generated from the Rust `TuningSettings` wire contract. The page
 * deliberately submits the complete typed object: config validation stays in
 * the runtime and one accepted snapshot cannot contain a mixed old/new limit.
 */
import { useEffect, useMemo, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import type { TuningSettings } from '../../../shared/contracts'
import type { SettingsApi } from '../hooks/use-settings'
import { describeOutcome } from '../api/outcome'
import { Field } from './Field'
import { Notice, SettingsSection, Skeleton } from './SectionShell'

type TuningKey = keyof TuningSettings

type TuningField = {
  key: TuningKey
  labelKey: MessageKey
  hintKey: MessageKey
  min: number
  max: number
}

const FIELDS: readonly TuningField[] = [
  { key: 'max_rounds', labelKey: 'settings.tuning.field.maxRounds.label', hintKey: 'settings.tuning.field.maxRounds.hint', min: 1, max: 200 },
  { key: 'max_tokens', labelKey: 'settings.tuning.field.maxTokens.label', hintKey: 'settings.tuning.field.maxTokens.hint', min: 1_000, max: 50_000_000 },
  { key: 'max_tool_calls', labelKey: 'settings.tuning.field.maxToolCalls.label', hintKey: 'settings.tuning.field.maxToolCalls.hint', min: 1, max: 5_000 },
  { key: 'max_identical_calls', labelKey: 'settings.tuning.field.maxIdenticalCalls.label', hintKey: 'settings.tuning.field.maxIdenticalCalls.hint', min: 1, max: 100 },
  { key: 'context_limit_tokens', labelKey: 'settings.tuning.field.contextLimitTokens.label', hintKey: 'settings.tuning.field.contextLimitTokens.hint', min: 1_000, max: 10_000_000 },
  { key: 'keep_recent_messages', labelKey: 'settings.tuning.field.keepRecentMessages.label', hintKey: 'settings.tuning.field.keepRecentMessages.hint', min: 1, max: 10_000 },
  { key: 'chunk_buffer', labelKey: 'settings.tuning.field.chunkBuffer.label', hintKey: 'settings.tuning.field.chunkBuffer.hint', min: 1, max: 4_096 },
  { key: 'sse_heartbeat_secs', labelKey: 'settings.tuning.field.sseHeartbeatSecs.label', hintKey: 'settings.tuning.field.sseHeartbeatSecs.hint', min: 0, max: 3_600 },
  { key: 'sse_idle_timeout_secs', labelKey: 'settings.tuning.field.sseIdleTimeoutSecs.label', hintKey: 'settings.tuning.field.sseIdleTimeoutSecs.hint', min: 0, max: 86_400 }
]

function cloneTuning(value: TuningSettings): TuningSettings {
  return { ...value }
}

function validDraft(draft: TuningSettings): boolean {
  return FIELDS.every(({ key, min, max }) => {
    const value = draft[key]
    return Number.isInteger(value) && value >= min && value <= max
  })
}

export function TuningSection({ settings }: { settings: SettingsApi }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const { config, saving, save } = settings
  const [draft, setDraft] = useState<TuningSettings | null>(null)
  const [rejection, setRejection] = useState<string | null>(null)
  const [defaultAutonomy, setDefaultAutonomy] = useState('workspace_write')

  useEffect(() => {
    if (config) {
      setDraft(cloneTuning(config.tuning))
      setDefaultAutonomy(config.default_autonomy)
    }
  }, [config])

  const valid = draft !== null && validDraft(draft)
  const dirty = useMemo(() => {
    if (!config || !draft) return false
    return FIELDS.some(({ key }) => draft[key] !== config.tuning[key])
  }, [config, draft])

  if (!config || !draft) {
    return (
      <SettingsSection title={t('settings.tuning.title')} desc={t('settings.tuning.descLoading')}>
        <Skeleton rows={5} />
      </SettingsSection>
    )
  }

  const setValue = (key: TuningKey, raw: string): void => {
    const next = Number(raw)
    setDraft((current) => (current ? { ...current, [key]: next } : current))
  }

  const persist = async (): Promise<void> => {
    if (!valid) return
    setRejection(null)
    const outcome = await save({ tuning: draft }, 'tuning')
    if (!outcome.ok) {
      toast.push('error', t('settings.tuning.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setRejection(outcome.value.reason)
      toast.push('error', t('settings.tuning.rejected'), outcome.value.reason)
      return
    }
    toast.push('success', t('settings.tuning.saved'), outcome.value.changed.join(t('common.listSeparator')) || t('settings.tuning.noChange'))
  }

  const persistDefaultAutonomy = async (): Promise<void> => {
    const outcome = await save({ default_autonomy: defaultAutonomy }, 'default_autonomy')
    if (!outcome.ok) {
      toast.push('error', t('settings.autonomy.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setRejection(outcome.value.reason)
      return
    }
    toast.push('success', t('settings.autonomy.saved'))
  }

  return (
    <SettingsSection title={t('settings.tuning.title')} desc={t('settings.tuning.desc')}>
      <div className="ark-settings__grid">
        {FIELDS.map((field) => {
          const value = draft[field.key]
          const invalid = !Number.isInteger(value) || value < field.min || value > field.max
          return (
            <Field
              key={field.key}
              label={t(field.labelKey)}
              htmlFor={`tuning-${field.key}`}
              hint={`${t(field.hintKey)} ${t('settings.tuning.rangeSuffix', { min: field.min, max: field.max })}`}
            >
              <input
                id={`tuning-${field.key}`}
                className="ark-field__input"
                type="number"
                min={field.min}
                max={field.max}
                step={1}
                value={Number.isFinite(value) ? value : ''}
                disabled={saving === 'tuning'}
                aria-invalid={invalid}
                onChange={(event) => setValue(field.key, event.target.value)}
              />
            </Field>
          )
        })}
      </div>

      <Field label={t('settings.autonomy.label')} htmlFor="default-autonomy" hint={t('settings.autonomy.hint')}>
        <select id="default-autonomy" className="ark-field__input" value={defaultAutonomy} disabled={saving === 'default_autonomy'} onChange={(event) => setDefaultAutonomy(event.target.value)}>
          <option value="strict">{t('session.runtime.strict')}</option>
          <option value="workspace_write">{t('session.runtime.workspaceWrite')}</option>
          <option value="unattended">{t('session.runtime.unattended')}</option>
        </select>
      </Field>

      {!valid ? <Notice tone="error" title={t('settings.tuning.invalidTitle')}>{t('settings.tuning.invalidBody')}</Notice> : null}
      {rejection ? <Notice tone="error" title={t('settings.tuning.rejectedTitle')}>{rejection}</Notice> : null}

      <div className="ark-settings__actions">
        <Button variant="primary" size="sm" disabled={defaultAutonomy === config.default_autonomy || saving === 'default_autonomy'} onClick={() => void persistDefaultAutonomy()}>
          {saving === 'default_autonomy' ? t('settings.autonomy.saving') : t('settings.autonomy.save')}
        </Button>
        <Button variant="primary" size="sm" disabled={!valid || !dirty || saving === 'tuning'} onClick={() => void persist()}>
          {saving === 'tuning' ? t('settings.tuning.saving') : t('settings.tuning.saveTuning')}
        </Button>
        <Button size="sm" disabled={!dirty || saving === 'tuning'} onClick={() => setDraft(cloneTuning(config.tuning))}>
          {t('settings.tuning.revert')}
        </Button>
      </div>
    </SettingsSection>
  )
}
