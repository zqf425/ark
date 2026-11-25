/**
 * Provider list + create/edit form, shown as a master-detail surface.
 *
 * The left column only renders the redacted list view (`has_api_key` /
 * `api_key_hint`). Selecting a row opens its full read model on the right;
 * opening the edit form explicitly reads the full entry, including the stored
 * api key, so the field is prefilled instead of looking lost.
 *
 * Saving never flips the global default: the default decides which provider
 * every default-following session resolves to, so it can only be changed
 * deliberately (detail action or Settings -> General), never as a side effect
 * of adding a provider.
 */
import { useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type {
  ProviderEntry,
  ProviderModelCatalogRequest,
  ProviderModelCatalogResponse,
  ProviderTestResponse,
  ProviderView
} from '../../../shared/contracts'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { ProviderForm } from './ProviderForm'
import { draftFromView, emptyDraft, type ProviderDraft } from './ProviderDraft'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

type TestState = { key: string; result?: ProviderTestResponse; error?: string }

function ProviderNavItem({
  provider,
  active,
  isDefault,
  isActive,
  onSelect
}: {
  provider: ProviderView
  active: boolean
  isDefault: boolean
  isActive: boolean
  onSelect: () => void
}): JSX.Element {
  const { t } = useI18n()
  return (
    <button type="button" className={`ark-provider-nav${active ? ' is-active' : ''}`} onClick={onSelect} aria-current={active ? 'true' : undefined}>
      <span className={`ark-provider-nav__dot${provider.enabled ? ' is-on' : ''}`} />
      <span className="ark-provider-nav__copy">
        <strong>{provider.id}</strong>
        <small>{provider.model} · {provider.endpoint_format}</small>
      </span>
      {isDefault ? <span className="ark-chip ark-chip--ok">{t('settings.providers.isDefault')}</span> : null}
      {isActive ? <span className="ark-chip ark-chip--live">{t('settings.general.defaultModel.active')}</span> : null}
      {provider.enabled ? null : <span className="ark-chip ark-chip--muted">{t('settings.providers.disabled')}</span>}
    </button>
  )
}

function DetailSection({ title, children }: { title: string; children: React.ReactNode }): JSX.Element {
  return (
    <section className="ark-provider-detail__section">
      <h3>{title}</h3>
      {children}
    </section>
  )
}

function ProviderDetail({
  provider,
  api,
  testState,
  onEdit,
  onTest,
  onMakeDefault,
  onRemove
}: {
  provider: ProviderView
  api: ProvidersApi
  testState: TestState | null
  onEdit: () => void
  onTest: () => void
  onMakeDefault: () => void
  onRemove: () => void
}): JSX.Element {
  const { t } = useI18n()
  const isDefault = api.defaultProviderId === provider.id || provider.is_default
  const isActive = api.activeProviderId === provider.id
  const shown = testState?.key === `test:${provider.id}` ? testState : null
  const apiKey = provider.has_api_key
    ? provider.api_key_hint
      ? t('settings.providers.apiKeyConfigured', { hint: provider.api_key_hint })
      : t('settings.providers.apiKeyConfiguredPlain')
    : t('settings.providers.apiKeyMissing')
  const models = [provider.model, ...(provider.models ?? []).map((row) => row.id)].filter(Boolean)

  return (
    <div className="ark-card ark-provider-detail">
      <header className="ark-provider-detail__head">
        <div>
          <h2>{provider.id}</h2>
          <p>{provider.label ?? t('settings.providers.detailHint')}</p>
        </div>
        <div className="ark-card__actions">
          <Button size="sm" disabled={api.busy !== null} onClick={onTest}>{t('common.test')}</Button>
          <Button size="sm" disabled={api.busy !== null || isDefault} onClick={onMakeDefault}>
            {isDefault ? t('settings.providers.alreadyDefault') : t('settings.providers.makeDefault')}
          </Button>
          <Button size="sm" disabled={api.busy !== null} onClick={onEdit}>{t('common.edit')}</Button>
          <Button variant="danger" size="sm" disabled={api.busy !== null} onClick={onRemove}>{t('common.delete')}</Button>
        </div>
      </header>

      <DetailSection title={t('settings.providers.section.connection')}>
        <div className="ark-provider-kv">
          <span>base_url</span><code>{provider.base_url}</code>
          <span>endpoint_format</span><code>{provider.endpoint_format}</code>
          <span>api_key</span><span>{apiKey}</span>
          <span>{t('settings.general.defaultModel.active')}</span>
          <span>
            {isActive ? <span className="ark-chip ark-chip--live">{t('settings.general.defaultModel.active')}</span> : null}
            {isDefault ? <span className="ark-chip ark-chip--ok">{t('settings.providers.isDefault')}</span> : null}
            {provider.enabled ? null : <span className="ark-chip ark-chip--muted">{t('settings.providers.disabled')}</span>}
          </span>
        </div>
        <p className="ark-settings__hint">{t('settings.providers.apiKeyReadonlyHint')}</p>
      </DetailSection>

      <DetailSection title={t('settings.providers.section.defaultModel')}>
        <div className="ark-provider-kv">
          <span>{t('settings.providerForm.defaultModelLabel')}</span><code>{provider.model}</code>
          <span>{t('settings.providerForm.inventoryContextPlaceholder')}</span>
          <span>{provider.context_window_tokens ?? '—'}</span>
          <span>{t('settings.providers.visionLabel')}</span>
          <span>{provider.supports_vision ? t('settings.providers.visionSupported') : t('settings.providers.visionUnsupported')}</span>
          <span>{t('settings.providers.detailScope')}</span><span>{t('settings.general.defaultModel.activeHint')}</span>
        </div>
      </DetailSection>

      <DetailSection title={t('settings.providers.section.models')}>
        <div className="ark-provider-models">
          {models.map((id, index) => (
            <span className="ark-chip" key={`${id}:${index}`}>{id}{index === 0 ? ` · ${t('settings.providerForm.defaultModelBadge')}` : ''}</span>
          ))}
        </div>
        <p className="ark-settings__hint">{t('settings.providers.editModelsHint')}</p>
      </DetailSection>

      <DetailSection title={t('settings.providers.section.lastTest')}>
        {shown?.error ? <Notice tone="error" title={t('settings.providers.testRequestFailed')}>{shown.error}</Notice> : null}
        {shown?.result ? (
          <Notice tone={shown.result.ok ? 'warn' : 'error'} title={shown.result.ok ? t('settings.providers.testPassed') : t('settings.providers.testNotPassed')}>
            <span className="ark-card__meta">
              <span>ok: {String(shown.result.ok)}</span>
              <span>status: {shown.result.status ?? '—'}</span>
              <span>latency: {shown.result.latency_ms} ms</span>
            </span>
            <span>{shown.result.message}</span>
            {shown.result.request_url ? <code className="ark-settings__code">{shown.result.request_url}</code> : null}
          </Notice>
        ) : <p className="ark-settings__hint">{t('settings.providers.noTestYet')}</p>}
      </DetailSection>
    </div>
  )
}

export type ProvidersSectionProps = { providers: ProvidersApi }

export function ProvidersSection({ providers: api }: ProvidersSectionProps): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const [draft, setDraft] = useState<ProviderDraft | null>(null)
  const [editing, setEditing] = useState(false)
  const [testState, setTestState] = useState<TestState | null>(null)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const selected = api.providers.find((row) => row.id === selectedId) ?? api.providers[0] ?? null

  const close = (): void => {
    setDraft(null)
    setEditing(false)
  }

  const submit = async (entry: ProviderEntry): Promise<void> => {
    const outcome = await api.upsert({ ...entry, make_default: false })
    if (outcome.ok) {
      setSelectedId(entry.id)
      toast.push('success', t('settings.providers.saved', { id: entry.id }))
      close()
      return
    }
    toast.push('error', t('settings.providers.saveFailed'), describeOutcome(outcome))
  }

  const openEdit = async (view: ProviderView): Promise<void> => {
    setSelectedId(view.id)
    const outcome = await api.get(view.id)
    if (!outcome.ok) {
      toast.push('error', t('settings.providers.loadFailed'), describeOutcome(outcome))
      return
    }
    setEditing(true)
    setDraft(draftFromView(view, outcome.value.api_key ?? ''))
  }

  const runTest = async (request: { provider_id?: string; entry?: ProviderEntry }, key: string): Promise<void> => {
    setTestState({ key })
    const outcome = await api.test(request)
    if (outcome.ok) {
      setTestState({ key, result: outcome.value })
      if (!outcome.value.ok) toast.push('error', t('settings.providers.testFailedToast'), outcome.value.message)
      else toast.push('success', t('settings.providers.testPassedToast'), `${outcome.value.latency_ms} ms`)
      return
    }
    setTestState({ key, error: describeOutcome(outcome) })
    toast.push('error', t('settings.providers.testRequestFailed'), describeOutcome(outcome))
  }

  const discover = async (request: ProviderModelCatalogRequest): Promise<ProviderModelCatalogResponse> => {
    const outcome = await api.models(request)
    if (outcome.ok) {
      toast.push('success', t('settings.providers.catalogUpdated'), t('settings.providers.catalogCount', { count: outcome.value.models.length }))
      return outcome.value
    }
    const message = describeOutcome(outcome)
    toast.push('error', t('settings.providers.catalogQueryFailed'), message)
    throw new Error(message)
  }

  const setDefault = async (): Promise<void> => {
    if (!selected) return
    const outcome = await api.setDefault(selected.id)
    if (outcome.ok) toast.push('success', t('settings.providers.setDefaultDone', { id: selected.id }))
    else toast.push('error', t('settings.providers.setDefaultFailed'), describeOutcome(outcome))
  }

  const remove = async (): Promise<void> => {
    if (!selected) return
    const outcome = await api.remove(selected.id)
    if (outcome.ok) {
      toast.push('success', t('settings.providers.deleted', { id: selected.id }))
      setSelectedId(null)
      close()
    } else toast.push('error', t('settings.providers.deleteFailed'), describeOutcome(outcome))
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.providers.length === 0) return <Skeleton rows={3} />
    if (api.status === 'error') return <LoadError message={api.error ?? t('settings.providers.loadFailed')} onRetry={() => void api.refresh()} />
    if (api.providers.length === 0) return <EmptyState title={t('settings.providers.emptyTitle')} hint={t('settings.providers.emptyHint')} />
    return (
      <div className="ark-provider-master">
        <div className="ark-provider-nav-wrap" role="group" aria-label={t('settings.providers.listAria')}>
          {api.providers.map((provider) => (
            <ProviderNavItem
              key={provider.id}
              provider={provider}
              active={selected?.id === provider.id}
              isDefault={api.defaultProviderId === provider.id || provider.is_default}
              isActive={api.activeProviderId === provider.id}
              onSelect={() => { setSelectedId(provider.id); close() }}
            />
          ))}
        </div>
        <div className="ark-provider-detail-wrap">
          {draft ? (
            <ProviderForm
              key={`${editing ? 'edit' : 'create'}:${draft.id || 'new'}`}
              editing={editing}
              initial={draft}
              busy={api.busy !== null}
              catalogBusy={api.catalogBusy}
              onCancel={close}
              onSubmit={submit}
              onTest={(entry) => runTest({ entry }, `test:${entry.id}`)}
              onDiscover={discover}
            />
          ) : selected ? (
            <ProviderDetail
              provider={selected}
              api={api}
              testState={testState}
              onEdit={() => void openEdit(selected)}
              onTest={() => void runTest({ provider_id: selected.id }, `test:${selected.id}`)}
              onMakeDefault={() => void setDefault()}
              onRemove={() => void remove()}
            />
          ) : <EmptyState title={t('settings.providers.detailEmptyTitle')} hint={t('settings.providers.detailEmptyHint')} />}
        </div>
      </div>
    )
  })()

  return (
    <SettingsSection
      title={t('settings.providers.title')}
      desc={t('settings.providers.desc')}
      actions={
        draft ? null : (
          <Button variant="primary" size="sm" disabled={api.busy !== null} onClick={() => { setEditing(false); setDraft(emptyDraft()) }}>
            {t('settings.providers.add')}
          </Button>
        )
      }
    >
      {api.defaultProviderId !== api.activeProviderId && api.activeProviderId ? (
        <p className="ark-settings__hint ark-settings__hint--warn">
          {t('settings.providers.defaultActiveMismatch', { defaultId: api.defaultProviderId ?? t('common.notSet'), activeId: api.activeProviderId })}
        </p>
      ) : null}
      {body}
    </SettingsSection>
  )
}
