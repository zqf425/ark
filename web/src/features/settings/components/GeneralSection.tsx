/**
 * 通用 section: default model, default workspace, log level, read-only facts.
 *
 * The section never calls fetch directly: it composes `useSettings` (config) and
 * `useProviders` (default model choice) and reports results through the shared
 * Toast provider.
 */
import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n, type UiFontSize, type UiLocale, type UiTheme } from '../../../shared/i18n'
import { LOG_LEVELS, type LogLevel } from '../../../shared/contracts'
import { useSettings, useWorkspaceProbe } from '../hooks/use-settings'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { Field, SelectField } from './Field'
import { KeyValue, LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

const LOG_LEVEL_LABELS: Record<LogLevel, string> = {
  trace: 'trace（最详细）',
  debug: 'debug',
  info: 'info',
  warn: 'warn',
  error: 'error',
  off: 'off（关闭日志）'
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
  const toast = useToast()
  const [pending, setPending] = useState(false)

  if (providers.status === 'loading' && providers.providers.length === 0) return <Skeleton rows={2} />
  if (providers.status === 'error') return <LoadError message={providers.error ?? '加载供应商失败'} onRetry={() => void providers.refresh()} />

  const disabled = providers.providers.length === 0 || pending || providers.busy !== null

  const change = async (providerId: string): Promise<void> => {
    if (!providerId) return
    setPending(true)
    const outcome = await providers.setDefault(providerId)
    setPending(false)
    if (outcome.ok) toast.push('success', `默认模型已切换为 ${providerId}`)
    else toast.push('error', '设置默认模型失败', describeOutcome(outcome))
  }

  return (
    <>
      <Field label="默认模型" htmlFor="settings-default-provider" hint="选择即时生效：写入为 default provider。">
        <select
          id="settings-default-provider"
          className="ark-field__select"
          value={providers.defaultProviderId ?? ''}
          disabled={disabled}
          onChange={(event) => void change(event.target.value)}
        >
          <option value="">{providers.providers.length === 0 ? '（暂无供应商）' : '（未设置）'}</option>
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
                <span className="ark-chip ark-chip--live">真正在用</span>
              </span>
            )
          }
        ]}
      />
      <p className="ark-settings__hint">
        active 表示当前实际发起请求的供应商；default 只是配置目标，运行中的会话可能要等到下一轮才切换。
      </p>
    </>
  )
}

/** Workspace input + validate + status probe. */
function WorkspaceEditor({ initial, onSave, saving }: { initial: string; onSave: (value: string) => Promise<void>; saving: boolean }): JSX.Element {
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
      toast.push('error', '请先填写工作空间路径')
      return
    }
    setValidating(true)
    const outcome = await probe.validate(path)
    setValidating(false)
    if (outcome.ok) toast.push('success', '路径校验通过', outcome.value.path)
    else toast.push('error', '路径校验失败', describeOutcome(outcome))
  }

  const status = probe.status

  return (
    <>
      <Field
        label="默认工作空间"
        htmlFor="settings-workspace"
        hint="留空并保存 = 清除默认工作空间（提交空字符串）。"
      >
        <div className="ark-field__inline">
          <input
            id="settings-workspace"
            className="ark-field__input"
            value={draft}
            spellCheck={false}
            autoComplete="off"
            placeholder="D:\\code\\project 或 /home/me/project"
            style={{ fontFamily: 'var(--ark-mono)', fontSize: 11.5 }}
            onChange={(event) => setDraft(event.target.value)}
          />
          <Button size="sm" disabled={validating} onClick={() => void validate()}>
            {validating ? '校验中…' : '校验'}
          </Button>
          <Button variant="primary" size="sm" disabled={saving} onClick={() => void onSave(draft.trim())}>
            {saving ? '保存中…' : '保存'}
          </Button>
        </div>
      </Field>

      {probe.error ? <Notice tone="error" title="工作空间状态读取失败">{probe.error}</Notice> : null}

      {status ? (
        <KeyValue
          rows={[
            { key: 'path', value: <code className="ark-settings__code">{status.path}</code> },
            { key: 'exists', value: status.exists ? <span className="ark-chip ark-chip--ok">存在</span> : <span className="ark-chip ark-chip--error">不存在</span> },
            { key: 'git 仓库', value: status.is_git_repository ? <span className="ark-chip ark-chip--ok">是</span> : <span className="ark-chip ark-chip--muted">否</span> },
            ...(status.is_git_repository
              ? [
                  { key: 'branch', value: <code className="ark-settings__code">{status.branch ?? '—'}</code> },
                  { key: 'head', value: <code className="ark-settings__code">{status.head_sha ? status.head_sha.slice(0, 12) : '—'}</code> },
                  {
                    key: 'dirty',
                    value:
                      status.is_dirty === true ? (
                        <span className="ark-chip ark-chip--error">有未提交改动</span>
                      ) : status.is_dirty === false ? (
                        <span className="ark-chip ark-chip--ok">干净</span>
                      ) : (
                        '—'
                      )
                  },
                  { key: '变更数', value: String(status.file_change_count ?? 0) }
                ]
              : [])
          ]}
        />
      ) : (
        <p className="ark-settings__hint">{probe.loading ? '读取工作空间状态…' : '点「校验」可读取该路径的存在性、git 分支与改动状态。'}</p>
      )}
    </>
  )
}

export type GeneralSectionProps = {
  settings: ReturnType<typeof useSettings>
  providers: ProvidersApi
}

export function GeneralSection({ settings, providers }: GeneralSectionProps): JSX.Element {
  const toast = useToast()
  const { config, status, error, saving, refresh, save } = settings

  const savePatch = async (patch: Parameters<typeof save>[0], key: string, okMessage: string): Promise<void> => {
    const outcome = await save(patch, key)
    if (!outcome.ok) {
      toast.push('error', '保存失败', describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      // A rejection is a normal result: never collapse it into a generic toast.
      toast.push('error', '运行时拒绝了该配置', outcome.value.reason)
      return
    }
    toast.push('success', okMessage, outcome.value.changed.length > 0 ? `已变更：${outcome.value.changed.join('、')}` : undefined)
  }

  if ((status === 'loading' || status === 'idle') && !config) {
    return (
      <>
        <AppearancePreferences />
        <SettingsSection title="通用" desc="默认模型、工作空间与日志级别。">
          <Skeleton rows={5} />
        </SettingsSection>
      </>
    )
  }

  if (status === 'error' && !config) {
    return (
      <>
        <AppearancePreferences />
        <SettingsSection title="通用" desc="默认模型、工作空间与日志级别。">
          <Notice tone="error" title="运行时配置读取失败">
            {error ?? '未知错误'}
          </Notice>
          <div className="ark-settings__actions">
            <Button size="sm" onClick={() => void refresh()}>
              重试
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
      <SettingsSection title="通用" desc="默认模型、工作空间与日志级别。">
        <DefaultModelPicker providers={providers} />

      {config ? (
        <WorkspaceEditor
          initial={config.default_workspace ?? ''}
          saving={saving === 'default_workspace'}
          onSave={(value) => savePatch({ default_workspace: value }, 'default_workspace', value ? '默认工作空间已保存' : '默认工作空间已清除')}
        />
      ) : null}

      <SelectField<LogLevel>
        id="settings-log-level"
        label="日志级别"
        value={logLevel}
        options={LOG_LEVELS}
        labels={LOG_LEVEL_LABELS}
        disabled={!config || saving === 'log_level'}
        hint="保存后立即生效（runtime 内部热切换）。"
        onChange={(next) => void savePatch({ log_level: next }, 'log_level', `日志级别已设为 ${next}`)}
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
