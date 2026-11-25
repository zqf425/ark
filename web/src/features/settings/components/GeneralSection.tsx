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
import { isDesktopShell, pickFolder } from '../../../shared/desktop'
import { LOG_LEVELS, type LogLevel } from '../../../shared/contracts'
import { useSettings, useWorkspaceProbe } from '../hooks/use-settings'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { Field, SelectField } from './Field'
import { ModelProviderMenu, toDefaultModelGroups } from '../../../shared/model-provider-menu'
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
      {/* Three independent device-local preferences share one row. */}
      <div className="ark-settings__grid ark-settings__grid--appearance">
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
      </div>
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

  const groups = toDefaultModelGroups(providers.providers)
  const currentProvider = providers.providers.find((row) => row.id === providers.defaultProviderId)
  // A pinned default model wins; without one the provider's stored `model` is
  // what a turn actually resolves, so the picker previews that.
  const currentModel = providers.defaultModel ?? currentProvider?.model ?? ''

  const change = async (selection: { providerId: string; model: string }): Promise<void> => {
    const providerId = selection.providerId.trim()
    const model = selection.model.trim()
    if (!providerId) return
    // Re-picking the exact stored pair is a no-op; switching provider without a
    // model (blank) resets that provider to its stored `model`.
    if (providerId === providers.defaultProviderId && model === (providers.defaultModel ?? '')) return
    setPending(true)
    const outcome = await providers.setDefault(providerId, model || undefined)
    setPending(false)
    if (outcome.ok)
      toast.push(
        'success',
        t('settings.general.defaultModel.switched', { providerId, model: model || currentProvider?.model || '' })
      )
    else toast.push('error', t('settings.general.defaultModel.switchFailed'), describeOutcome(outcome))
  }

  return (
    <>
      <Field label={t('settings.general.defaultModel.label')} htmlFor="settings-default-provider" hint={t('settings.general.defaultModel.hint')}>
        <ModelProviderMenu
          id="settings-default-provider"
          groups={groups}
          value={{ providerId: providers.defaultProviderId ?? '', model: currentModel }}
          disabled={disabled}
          ariaLabel={t('settings.general.defaultModel.label')}
          placeholder={
            providers.providers.length === 0
              ? t('settings.general.defaultModel.noProviders')
              : t('settings.general.defaultModel.unset')
          }
          onSelect={(selection) => void change(selection)}
        />
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

/** Read-only workspace display + single change button (native folder picker). */
function WorkspaceEditor({ initial, onSave, saving }: { initial: string; onSave: (value: string) => Promise<void>; saving: boolean }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const probe = useWorkspaceProbe()
  const [browsing, setBrowsing] = useState(false)
  const desktop = isDesktopShell()

  // The stored default wins over any earlier probe result; refresh it on mount
  // and after every save so the status card tracks the actual config value.
  useEffect(() => {
    const path = initial.trim()
    if (path) void probe.probe(path)
  }, [initial, probe])

  const browse = async (): Promise<void> => {
    setBrowsing(true)
    try {
      const folder = await pickFolder()
      // null = cancelled or non-desktop environment: keep the current value.
      if (folder) await onSave(folder)
    } catch {
      toast.push('error', t('settings.general.workspace.pickFailed'))
    } finally {
      setBrowsing(false)
    }
  }

  const status = probe.status
  const current = initial.trim()

  return (
    <>
      <Field label={t('settings.general.workspace.label')} hint={t('settings.general.workspace.hint')}>
        <div className="ark-field__inline">
          {current ? (
            <code className="ark-settings__code ark-settings__code--fill">{current}</code>
          ) : (
            <span className="ark-settings__hint">{t('settings.general.workspace.unset')}</span>
          )}
          {desktop ? (
            <Button size="sm" disabled={browsing || saving} onClick={() => void browse()}>
              {browsing ? t('settings.general.workspace.browsing') : t('settings.general.workspace.change')}
            </Button>
          ) : null}
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
        <p className="ark-settings__hint">{probe.loading ? t('settings.general.workspace.reading') : null}</p>
      )}
    </>
  )
}

export type GeneralSectionProps = {
  settings: ReturnType<typeof useSettings>
  providers: ProvidersApi
}

/**
 * 通用页实现级布局（与 .tmp/settings-ui-implementation-demo.html 对齐）：
 *   1) 默认模型
 *   2) 工作区
 *   3) 外观与语言
 * 其余运行时只读信息与日志级别收在第一张卡片的尾部，不新增全局假按钮。
 */
export function GeneralSection({ settings, providers }: GeneralSectionProps): JSX.Element {
  const { t } = useI18n()
  const { config, status, error, saving, refresh, save } = settings

  const savePatch = async (patch: Parameters<typeof save>[0], key: string, okMessage: string): Promise<void> => {
    const outcome = await save(patch, key)
    // The hook owns persistence; failures are surfaced inside the card so a
    // general page never pretends a rejected patch was applied.
    if (!outcome.ok) void outcome
    else if (outcome.value.result === 'applied') void okMessage
  }

  if ((status === 'loading' || status === 'idle') && !config) {
    return (
      <>
        <SettingsSection title={t('settings.general.defaultModel.label')} desc={t('settings.general.defaultModel.hint')}>
          <Skeleton rows={3} />
        </SettingsSection>
        <SettingsSection title={t('settings.general.workspace.label')} desc={t('settings.general.workspace.hint')}>
          <Skeleton rows={3} />
        </SettingsSection>
        <AppearancePreferences />
      </>
    )
  }

  return (
    <>
      <SettingsSection title={t('settings.general.defaultModel.label')} desc={t('settings.general.defaultModel.hint')}>
        <DefaultModelPicker providers={providers} />
      </SettingsSection>

      <SettingsSection title={t('settings.general.workspace.label')} desc={t('settings.general.workspace.hint')}>
        {config ? (
          <WorkspaceEditor
            initial={config.default_workspace ?? ''}
            saving={saving === 'default_workspace'}
            onSave={(value) =>
              savePatch({ default_workspace: value }, 'default_workspace', t('settings.general.workspace.updated'))
            }
          />
        ) : (
          <Notice tone="error" title={t('settings.general.configLoadFailed')}>
            {error ?? t('settings.general.unknownError')}
          </Notice>
        )}
      </SettingsSection>

      <AppearancePreferences />

      <SettingsSection title={t('settings.general.title')} desc={t('settings.general.desc')}>
        <SelectField<LogLevel>
          id="settings-log-level"
          label={t('settings.general.logLevel.label')}
          value={config && isLogLevel(config.log_level) ? config.log_level : 'info'}
          options={LOG_LEVELS}
          labels={Object.fromEntries(LOG_LEVELS.map((level) => [level, t(LOG_LEVEL_LABEL_KEYS[level])])) as Record<LogLevel, string>}
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
        <div className="ark-settings__actions">
          <Button size="sm" onClick={() => void refresh()}>{t('common.retry')}</Button>
        </div>
      </SettingsSection>
    </>
  )
}
