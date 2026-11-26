/**
 * Read-only provider detail: header with primary actions, a secondary-action
 * menu, summary stats, connection, model table and the last test result.
 *
 * Destructive and global-routing actions (set default, delete) sit behind the
 * kebab menu so the header only carries the two everyday actions; deleting
 * always goes through a confirmation dialog owned by the parent section.
 */
import { useEffect, useRef, useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { EndpointFormat, ProviderTestResponse, ProviderView } from '../../../shared/contracts'
import { Notice } from './SectionShell'
import { useMenuFocus } from '../../../shared/ui/focus'

export type TestState = { key: string; result?: ProviderTestResponse; error?: string }

/**
 * Upstream path the runtime appends per endpoint format, kept without the
 * leading slash: these are provider-side paths, not arkd runtime routes, so
 * they must not look like one to the api-layer path rule.
 */
export const FORMAT_PATHS: Record<EndpointFormat, string> = {
  openai_chat: 'chat/completions',
  openai_responses: 'responses',
  anthropic_messages: 'v1/messages'
}

export const FORMAT_NAMES: Record<EndpointFormat, string> = {
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  anthropic_messages: 'Anthropic'
}

/** The view types endpoint_format as a plain string; unknown values pass through verbatim. */
export function formatName(format: string): string {
  return FORMAT_NAMES[format as EndpointFormat] ?? format
}

export function requestUrl(baseUrl: string, format: string): string {
  const path = FORMAT_PATHS[format as EndpointFormat]
  const base = baseUrl.trim().replace(/\/+$/, '')
  return path ? `${base}/${path}` : base
}

export function ProviderAvatar({ provider, large = false }: { provider: ProviderView; large?: boolean }): JSX.Element {
  const name = provider.label ?? provider.id
  return (
    <span className={`ark-provider-avatar${large ? ' ark-provider-avatar--lg' : ''}`} aria-hidden="true">
      {name.slice(0, 1).toUpperCase()}
      <i className={provider.enabled ? 'is-on' : ''} />
    </span>
  )
}

function formatTokens(value: number | null | undefined): string {
  if (!value) return '—'
  return value >= 1000 ? `${Math.round(value / 1000)}K` : String(value)
}

type ModelRowView = { id: string; context?: number | null; vision?: boolean | null; isDefault: boolean }

function modelRows(provider: ProviderView): ModelRowView[] {
  const rows: ModelRowView[] = [
    { id: provider.model, context: provider.context_window_tokens, vision: provider.supports_vision, isDefault: true }
  ]
  for (const item of provider.models ?? []) {
    if (item.id === provider.model) continue
    rows.push({ id: item.id, context: item.context_window_tokens, vision: item.supports_vision, isDefault: false })
  }
  return rows.filter((row) => row.id)
}

function MoreMenu({ disabled, isDefault, onMakeDefault, onRemove }: {
  disabled: boolean
  isDefault: boolean
  onMakeDefault: () => void
  onRemove: (trigger: HTMLButtonElement | null) => void
}): JSX.Element {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menu = useMenuFocus(open, () => setOpen(false), triggerRef)
  useEffect(() => {
    if (!open) return
    const close = (event: MouseEvent): void => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])
  const pick = (action: () => void) => (): void => { setOpen(false); action() }
  return (
    <div className="ark-provider-menu" ref={ref}>
      <Button ref={triggerRef} size="sm" disabled={disabled} aria-label={t('settings.providers.moreActions')} aria-expanded={open} onClick={() => setOpen((v) => !v)}>⋯</Button>
      {open ? (
        <div ref={menu.ref} onKeyDown={menu.onKeyDown} className="ark-provider-menu__pop" role="menu">
          <button type="button" role="menuitem" disabled={isDefault} onClick={pick(onMakeDefault)}>
            {isDefault ? t('settings.providers.alreadyDefault') : t('settings.providers.makeDefault')}
          </button>
          <button type="button" role="menuitem" className="is-danger" onClick={() => { setOpen(false); onRemove(triggerRef.current) }}>{t('common.delete')}…</button>
        </div>
      ) : null}
    </div>
  )
}

function LastTest({ shown }: { shown: TestState | null }): JSX.Element {
  const { t } = useI18n()
  if (shown?.error) return <Notice tone="error" title={t('settings.providers.testRequestFailed')}>{shown.error}</Notice>
  if (!shown?.result) return <p className="ark-settings__hint">{t('settings.providers.noTestYet')}</p>
  const r = shown.result
  return (
    <div className={`ark-provider-test${r.ok ? '' : ' is-fail'}`}>
      <span className={`ark-chip ${r.ok ? 'ark-chip--ok' : 'ark-chip--error'}`}>
        {r.ok ? t('settings.providers.testPassed') : t('settings.providers.testNotPassed')}
      </span>
      <span className="ark-provider-test__msg">HTTP {r.status ?? '—'} · {r.latency_ms} ms · {r.message}</span>
      {r.request_url ? <code className="ark-settings__code">{r.request_url}</code> : null}
    </div>
  )
}

function Stats({ provider, shown }: { provider: ProviderView; shown: TestState | null }): JSX.Element {
  const { t } = useI18n()
  const result = shown?.result
  const test = result
    ? result.ok ? t('settings.providers.stat.passed', { ms: result.latency_ms }) : t('settings.providers.stat.failed')
    : t('settings.providers.stat.untested')
  return (
    <div className="ark-provider-stats">
      <div><span>{t('settings.providers.stat.format')}</span><b>{formatName(provider.endpoint_format)}</b></div>
      <div><span>{t('settings.providers.stat.models')}</span><b>{t('settings.providers.stat.modelsCount', { count: modelRows(provider).length })}</b></div>
      <div><span>{t('settings.providers.stat.lastTest')}</span><b className={result ? (result.ok ? 'is-ok' : 'is-fail') : ''}>{test}</b></div>
    </div>
  )
}

function ModelTable({ provider }: { provider: ProviderView }): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-provider-table">
      <table>
        <thead><tr><th>{t('settings.providers.col.modelId')}</th><th>{t('settings.providers.col.context')}</th><th>{t('settings.providers.col.vision')}</th><th /></tr></thead>
        <tbody>
          {modelRows(provider).map((row) => (
            <tr key={row.id}>
              <td className="is-mono">{row.id}</td>
              <td>{formatTokens(row.context)}</td>
              <td>{row.vision ? '✓' : '—'}</td>
              <td>{row.isDefault ? <span className="ark-chip ark-chip--live">{t('settings.providerForm.defaultModelBadge')}</span> : null}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export type ProviderDetailProps = {
  provider: ProviderView
  busy: boolean
  isDefault: boolean
  testState: TestState | null
  onEdit: () => void
  onTest: () => void
  onMakeDefault: () => void
  onRemove: (trigger: HTMLButtonElement | null) => void
}

export function ProviderDetail({ provider, busy, isDefault, testState, onEdit, onTest, onMakeDefault, onRemove }: ProviderDetailProps): JSX.Element {
  const { t } = useI18n()
  const shown = testState?.key === `test:${provider.id}` ? testState : null
  const apiKey = provider.has_api_key
    ? <><span className="ark-chip ark-chip--ok">{t('settings.providers.apiKeyConfiguredPlain')}</span>{provider.api_key_hint ? <code>{provider.api_key_hint}</code> : null}</>
    : <span className="ark-chip ark-chip--warn">{t('settings.providers.apiKeyMissing')}</span>
  return (
    <div className="ark-card ark-provider-detail">
      <header className="ark-provider-detail__head">
        <ProviderAvatar provider={provider} large />
        <div className="ark-provider-detail__title">
          <h2>{provider.label ?? provider.id}{isDefault ? <span className="ark-chip ark-chip--ok">{t('settings.providers.isDefault')}</span> : null}</h2>
          <p>{provider.id}</p>
        </div>
        <Button size="sm" disabled={busy} onClick={onTest}>{t('settings.providers.testConnection')}</Button>
        <Button size="sm" disabled={busy} onClick={onEdit}>{t('common.edit')}</Button>
        <MoreMenu disabled={busy} isDefault={isDefault} onMakeDefault={onMakeDefault} onRemove={onRemove} />
      </header>
      <div className="ark-provider-detail__body">
        <Stats provider={provider} shown={shown} />
        <section className="ark-provider-detail__section">
          <h3>{t('settings.providers.section.connection')}</h3>
          <div className="ark-provider-kv">
            <span>{t('settings.providers.kv.baseUrl')}</span><code>{provider.base_url}</code>
            <span>{t('settings.providers.kv.requestUrl')}</span><code className="is-faint">{requestUrl(provider.base_url, provider.endpoint_format)}</code>
            <span>api_key</span><span className="ark-provider-kv__key">{apiKey}</span>
          </div>
        </section>
        <section className="ark-provider-detail__section">
          <h3>{t('settings.providers.section.models')}</h3>
          <ModelTable provider={provider} />
          <p className="ark-settings__hint">{t('settings.providers.editModelsHint')}</p>
        </section>
        <section className="ark-provider-detail__section">
          <h3>{t('settings.providers.section.lastTest')}</h3>
          <LastTest shown={shown} />
        </section>
      </div>
    </div>
  )
}
