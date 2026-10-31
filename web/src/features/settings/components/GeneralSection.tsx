/**
 * 通用 section: default model, default workspace, log level, read-only facts.
 *
 * The section never calls fetch directly: it composes `useSettings` (config) and
 * `useProviders` (default model choice) and reports results through the shared
 * Toast provider. Every user-visible string comes from the i18n catalog, so
 * switching the display language re-renders the section immediately.
 */
import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n, type MessageKey, type UiFontSize, type UiLocale, type UiTheme } from '../../../shared/i18n'
import { LOG_LEVELS, type LogLevel } from '../../../shared/contracts'
import { useSettings, useWorkspaceProbe } from '../hooks/use-settings'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { Field, SelectField } from './Field'
import { KeyValue, LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

/** Log level ids stay verbatim; only their explanatory suffix is translated. */
const LOG_LEVEL_LABEL_KEYS: Record<LogLevel, MessageKey> = {
  trace: 'settings.general.logLevel.trace',
  debug: 'settings.general.logLevel.debug',
  info: 'settings.general.logLevel.info',
  warn: 'settings.general.logLevel.warn',
  error: 'settings.general.logLevel.error',
  off: 'settings.general.logLevel.off'
}

function isLogLevel(value: string): value is LogLevel {
  return (LOG_LEVELS as readonly string[]).includes(value)
}

/** Renderer-only appearance preferences; never sent to the runtime configuration API. */
function AppearancePreferences(): JSX.Element {
  const { locale, fontSize, theme, setLocale, setFontSize, setTheme, t } = useI18n()
  return (
    <SettingsSection title={t('settings.appearance.title')} desc={t('settings.appearance.desc')}>
      <SelectField<UiLocale>
        id="settings-language"
        label={t('settings.language')}
        value={locale}
        options={['zh-CN', 'en-US']}
        labels={{ 'zh-CN': t('settings.language.zh-CN'), 'en-US': t('settings.language.en-US') }}
        onChange={setLocale}
      />
      <SelectField<UiFontSize>
        id="settings-font-size"
        label={t('settings.fontSize')}
        value={fontSize}
        options={['small', 'medium', 'large']}
        labels={{ small: t('settings.fontSize.small'), medium: t('settings.fontSize.medium'), large: t('settings.fontSize.large') }}
        onChange={setFontSize}
      />
      <SelectField<UiTheme>
        id="settings-theme"
        label={t('settings.theme')}
        value={theme}
        options={['light', 'dark', 'system']}
        labels={{ light: t('settings.theme.light'), dark: t('settings.theme.dark'), system: t('settings.theme.system') }}
        onChange={setTheme}
      />
    </SettingsSection>
  )
}

/** `active` is what the loop uses now; `default` is only the configured target. */
function DefaultModelPicker({ providers }: { providers: ProvidersApi }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const [pending, setPending] = useState(false)

  if (providers.status === 'loading' && providers.providers.length === 0) return <Skeleton rows={2} />
  if (providers.status === 'error') {
    return (
      <LoadError
        message={providers.error ?? t('settings.general.defaultModel.loadFailed')}
        onRetry={() => void providers.refresh()}
      />
    )
  }

  const disabled = providers.providers.length === 0 || pending || providers.busy !== null

  const change = async (providerId: string): Promise<void> => {
    if (!providerId) return
    setPending(true)
    const outcome = await providers.setDefault(providerId)
    setPending(false)
    if (outcome.ok) toast.push('success', t('settings.general.defaultModel.switched', { providerId }))
    else toast.push('error', t('settings.general.defaultModel.switchFailed'), describeOutcome(outcome))
  }

  return (
    <>
      <Field label={t('settings.general.defaultModel.label')} htmlFor="settings-default-provider" hint={t('settings.general.defaultModel.hint')}>
        <select
          id="settings-default-provider"
          className="ark-field__select"
          value={providers.defaultProviderId ?? ''}
          disabled={disabled}
          onChange={(event) => void change(event.target.value)}
        >
          <option value="">
            {providers.providers.length === 0
              ? t('settings.general.defaultModel.noProviders')
              : t('settings.general.defaultModel.unset')}
          </option>
          {providers.providers.map((provider) => (
            <option key={provider.id} value={provider.id}>
              {provider.label ? `${provider.label} · ${provider.id}` : provider.id}
              {provider.model ? ` · ${provider.model}` : ''}
            </option>
          ))}
        </select>
      </Field>

      <KeyValue
        rows={[
          { key: 'default_provider_id', value: <code className="ark-settings__code">{providers.defaultProviderId ?? '—'}</code> },
          {
            key: 'active_provider_id',
            value: (
              <span className="ark-settings__row">
                <code className="ark-settings__code">{providers.activeProviderId ?? '—'}</code>
                <span className="ark-chip ark-chip--live">{t('settings.general.defaultModel.active')}</span>
              </span>
            )
          }
        ]}
      />
      <p className="ark-settings__hint">{t('settings.general.defaultModel.activeHint')}</p>
    </>
  )
}

/** Workspace input + validate + status probe. */
function WorkspaceEditor({ initial, onSave, saving }: { initial: string; onSave: (value: string) => Promise<void>; saving: boolean }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const probe = useWorkspaceProbe()
  const [draft, setDraft] = useState(initial)
  const [validating, setValidating] = useState(false)

  // Config reloads (e.g. after a save) must not fight with an in-progress edit:
  // only adopt the new value when the field is untouched.
  useEffect(() => {
    setDraft((current) => (current.trim() === '' ? initial : current))
  }, [initial])

  const validate = async (): Promise<void> => {
    const path = draft.trim()
    if (!path) {
      toast.push('error', t('settings.general.workspace.emptyPath'))
      return
    }
    setValidating(true)
    const outcome = await probe.validate(path)
    setValidating(false)
    if (outcome.ok) toast.push('success', t('settings.general.workspace.validateOk'), outcome.value.path)
    else toast.push('error', t('settings.general.workspace.validateFailed'), describeOutcome(outcome))
  }

  const status = probe.status

  return (
    <>
      <Field label={t('settings.general.workspace.label')} htmlFor="settings-workspace" hint={t('settings.general.workspace.hint')}>
        <div className="ark-field__inline">
          <input
            id="settings-workspace"
            className="ark-field__input"
            value={draft}
            spellCheck={false}
            autoComplete="off"
            placeholder={t('settings.general.workspace.placeholder')}
            style={{ fontFamily: 'var(--ark-mono)', fontSize: 11.5 }}
            onChange={(event) => setDraft(event.target.value)}
          />
          <Button size="sm" disabled={validating} onClick={() => void validate()}>
            {validating ? t('settings.general.workspace.validating') : t('settings.general.workspace.validate')}
          </Button>
          <Button variant="primary" size="sm" disabled={saving} onClick={() => void onSave(draft.trim())}>
            {saving ? t('settings.general.workspace.saving') : t('settings.general.workspace.save')}
          </Button>
        </div>
      </Field>

      {probe.error ? <Notice tone="error" title={t('settings.general.workspace.statusFailed')}>{probe.error}</Notice> : null}

      {status ? (
        <KeyValue
          rows={[
            { key: 'path', value: <code className="ark-settings__code">{status.path}</code> },
            {
              key: 'exists',
              value: status.exists ? (
                <span className="ark-chip ark-chip--ok">{t('settings.general.workspace.exists')}</span>
              ) : (
                <span className="ark-chip ark-chip--error">{t('settings.general.workspace.missing')}</span>
              )
            },
            {
              key: t('settings.general.workspace.gitRepository'),
              value: status.is_git_repository ? (
                <span className="ark-chip ark-chip--ok">{t('settings.general.workspace.yes')}</span>
              ) : (
                <span className="ark-chip ark-chip--muted">{t('settings.general.workspace.no')}</span>
              )
            },
            ...(status.is_git_repository
              ? [
                  { key: 'branch', value: <code className="ark-settings__code">{status.branch ?? '—'}</code> },
                  { key: 'head', value: <code className="ark-settings__code">{status.head_sha ? status.head_sha.slice(0, 12) : '—'}</code> },
                  {
                    key: 'dirty',
                    value:
                      status.is_dirty === true ? (
                        <span className="ark-chip ark-chip--error">{t('settings.general.workspace.dirty')}</span>
                      ) : status.is_dirty === false ? (
                        <span className="ark-chip ark-chip--ok">{t('settings.general.workspace.clean')}</span>
                      ) : (
                        '—'
                      )
                  },
                  { key: t('settings.general.workspace.changeCount'), value: String(status.file_change_count ?? 0) }
                ]
              : [])
          ]}
        />
      ) : (
        <p className="ark-settings__hint">
          {probe.loading ? t('settings.general.workspace.reading') : t('settings.general.workspace.probeHint')}
        </p>
      )}
    </>
  )
}

export type GeneralSectionProps = {
  settings: ReturnType<typeof useSettings>
  providers: ProvidersApi
}

export function GeneralSection({ settings, providers }: GeneralSectionProps): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const { config, status, error, saving, refresh, save } = settings

  const savePatch = async (patch: Parameters<typeof save>[0], key: string, okMessage: string): Promise<void> => {
    const outcome = await save(patch, key)
    if (!outcome.ok) {
      toast.push('error', t('settings.general.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      // A rejection is a normal result: never collapse it into a generic toast.
      toast.push('error', t('settings.general.rejected'), outcome.value.reason)
      return
    }
    const changed = outcome.value.changed
    toast.push(
      'success',
      okMessage,
      changed.length > 0
        ? t('settings.general.changed', { keys: changed.join(t('settings.general.changedSeparator')) })
        : undefined
    )
  }

  const logLevelLabels = Object.fromEntries(
    LOG_LEVELS.map((level) => [level, t(LOG_LEVEL_LABEL_KEYS[level])])
  ) as Record<LogLevel, string>

  if ((status === 'loading' || status === 'idle') && !config) {
    return (
      <>
        <AppearancePreferences />
        <SettingsSection title={t('settings.general.title')} desc={t('settings.general.desc')}>
          <Skeleton rows={5} />
        </SettingsSection>
      </>
    )
  }

  if (status === 'error' && !config) {
    return (
      <>
        <AppearancePreferences />
        <SettingsSection title={t('settings.general.title')} desc={t('settings.general.desc')}>
          <Notice tone="error" title={t('settings.general.configLoadFailed')}>
            {error ?? t('settings.general.unknownError')}
          </Notice>
          <div className="ark-settings__actions">
            <Button size="sm" onClick={() => void refresh()}>
              {t('common.retry')}
            </Button>
          </div>
        </SettingsSection>
      </>
    )
  }

  const logLevel = config && isLogLevel(config.log_level) ? config.log_level : 'info'

  return (
    <>
      <AppearancePreferences />
      <SettingsSection title={t('settings.general.title')} desc={t('settings.general.desc')}>
        <DefaultModelPicker providers={providers} />

      {config ? (
        <WorkspaceEditor
          initial={config.default_workspace ?? ''}
          saving={saving === 'default_workspace'}
          onSave={(value) =>
            savePatch(
              { default_workspace: value },
              'default_workspace',
              value ? t('settings.general.workspace.saved') : t('settings.general.workspace.cleared')
            )
          }
        />
      ) : null}

      <SelectField<LogLevel>
        id="settings-log-level"
        label={t('settings.general.logLevel.label')}
        value={logLevel}
        options={LOG_LEVELS}
        labels={logLevelLabels}
        disabled={!config || saving === 'log_level'}
        hint={t('settings.general.logLevel.hint')}
        onChange={(next) => void savePatch({ log_level: next }, 'log_level', t('settings.general.logLevel.saved', { level: next }))}
      />

        {config ? (
          <KeyValue
            rows={[
              { key: 'version', value: <code className="ark-settings__code">v{config.version}</code> },
              { key: 'port', value: <code className="ark-settings__code">{config.port}</code> },
              { key: 'data_dir', value: <code className="ark-settings__code">{config.data_dir}</code> },
            ]}
          />
        ) : null}
      </SettingsSection>
    </>
  )
}
