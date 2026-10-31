/**
 * Provider list + create/edit form.
 *
 * The list only ever shows what the runtime returns: `has_api_key` and the
 * obfuscated `api_key_hint`. A plaintext key is never rendered, because the
 * runtime never sends one back.
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
import { draftFromView, emptyDraft, ProviderForm, type ProviderDraft } from './ProviderForm'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'

/** Pending test outcome, keyed by provider id (`*` = unsaved draft). */
type TestState = { key: string; result?: ProviderTestResponse; error?: string }

function ProviderRow({
  provider,
  api,
  onEdit,
  onTest,
  testState
}: {
  provider: ProviderView
  api: ProvidersApi
  onEdit: () => void
  onTest: () => void
  testState: TestState | null
}): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const active = api.activeProviderId === provider.id
  const isDefault = api.defaultProviderId === provider.id || provider.is_default

  const remove = async (): Promise<void> => {
    const outcome = await api.remove(provider.id)
    if (outcome.ok) toast.push('success', t('settings.providers.deleted', { id: provider.id }))
    else toast.push('error', t('settings.providers.deleteFailed'), describeOutcome(outcome))
  }

  const makeDefault = async (): Promise<void> => {
    const outcome = await api.setDefault(provider.id)
    if (outcome.ok) toast.push('success', t('settings.providers.setDefaultDone', { id: provider.id }))
    else toast.push('error', t('settings.providers.setDefaultFailed'), describeOutcome(outcome))
  }

  const shown = testState && testState.key === `test:${provider.id}` ? testState : null

  return (
    <div className="ark-card">
      <div className="ark-card__head">
        <span className="ark-card__name">{provider.id}</span>
        {provider.label ? <span className="ark-settings__hint">{provider.label}</span> : null}
        {isDefault ? <span className="ark-chip ark-chip--ok">{t('settings.providers.isDefault')}</span> : null}
        {active ? <span className="ark-chip ark-chip--live">active</span> : null}
        {provider.enabled ? null : <span className="ark-chip ark-chip--muted">{t('settings.providers.disabled')}</span>}
        <div className="ark-card__actions">
          <Button size="sm" disabled={api.busy !== null || isDefault} onClick={() => void makeDefault()}>
            {isDefault ? t('settings.providers.alreadyDefault') : t('settings.providers.makeDefault')}
          </Button>
          <Button size="sm" disabled={api.busy !== null} onClick={onTest}>
            {t('common.test')}
          </Button>
          <Button size="sm" disabled={api.busy !== null} onClick={onEdit}>
            {t('common.edit')}
          </Button>
          <Button variant="danger" size="sm" disabled={api.busy !== null} onClick={() => void remove()}>
            {t('common.delete')}
          </Button>
        </div>
      </div>

      <div className="ark-card__meta">
        <span>model: {provider.model}</span>
        <span>format: {provider.endpoint_format}</span>
        <span>base_url: {provider.base_url}</span>
        <span>
          api_key:{' '}
          {provider.has_api_key
            ? provider.api_key_hint
              ? t('settings.providers.apiKeyConfigured', { hint: provider.api_key_hint })
              : t('settings.providers.apiKeyConfiguredPlain')
            : t('settings.providers.apiKeyMissing')}
        </span>
        {provider.context_window_tokens ? <span>context: {provider.context_window_tokens}</span> : null}
        {provider.supports_vision !== undefined ? (
          <span>
            {t('settings.providers.visionLabel')}:{' '}
            {provider.supports_vision ? t('settings.providers.visionSupported') : t('settings.providers.visionUnsupported')}
          </span>
        ) : null}
      </div>

      {shown?.error ? <Notice tone="error" title={t('settings.providers.testRequestFailed')}>{shown.error}</Notice> : null}
      {shown?.result ? (
        <Notice
          tone={shown.result.ok ? 'warn' : 'error'}
          title={shown.result.ok ? t('settings.providers.testPassed') : t('settings.providers.testNotPassed')}
        >
          <span className="ark-card__meta">
            <span>ok: {String(shown.result.ok)}</span>
            <span>status: {shown.result.status ?? '—'}</span>
            <span>latency: {shown.result.latency_ms} ms</span>
          </span>
          <span>{shown.result.message}</span>
          {shown.result.request_url ? <code className="ark-settings__code">{shown.result.request_url}</code> : null}
        </Notice>
      ) : null}
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

  const close = (): void => {
    setDraft(null)
    setEditing(false)
  }

  const submit = async (entry: ProviderEntry, makeDefault: boolean): Promise<void> => {
    const outcome = await api.upsert({ ...entry, make_default: makeDefault })
    if (outcome.ok) {
      toast.push(
        'success',
        t('settings.providers.saved', { id: entry.id }),
        makeDefault ? t('settings.providers.savedAsDefault') : undefined
      )
      close()
      return
    }
    toast.push('error', t('settings.providers.saveFailed'), describeOutcome(outcome))
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
      toast.push(
        'success',
        t('settings.providers.catalogUpdated'),
        t('settings.providers.catalogCount', { count: outcome.value.models.length })
      )
      return outcome.value
    }
    const message = describeOutcome(outcome)
    toast.push('error', t('settings.providers.catalogQueryFailed'), message)
    throw new Error(message)
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.providers.length === 0) return <Skeleton rows={3} />
    if (api.status === 'error') {
      return <LoadError message={api.error ?? t('settings.providers.loadFailed')} onRetry={() => void api.refresh()} />
    }
    if (api.providers.length === 0) {
      return <EmptyState title={t('settings.providers.emptyTitle')} hint={t('settings.providers.emptyHint')} />
    }
    return (
      <div className="ark-settings__list">
        {api.providers.map((provider) => (
          <ProviderRow
            key={provider.id}
            provider={provider}
            api={api}
            testState={testState}
            onEdit={() => {
              setEditing(true)
              setDraft(draftFromView(provider))
            }}
            onTest={() => void runTest({ provider_id: provider.id }, `test:${provider.id}`)}
          />
        ))}
      </div>
    )
  })()

  return (
    <SettingsSection
      title={t('settings.providers.title')}
      desc={t('settings.providers.desc')}
      actions={
        draft ? null : (
          <Button
            variant="primary"
            size="sm"
            disabled={api.busy !== null}
            onClick={() => {
              setEditing(false)
              setDraft(emptyDraft())
            }}
          >
            {t('settings.providers.add')}
          </Button>
        )
      }
    >
      {api.defaultProviderId !== api.activeProviderId && api.activeProviderId ? (
        <p className="ark-settings__hint ark-settings__hint--warn">
          {t('settings.providers.defaultActiveMismatch', {
            defaultId: api.defaultProviderId ?? t('common.notSet'),
            activeId: api.activeProviderId
          })}
        </p>
      ) : null}

      {body}

      {draft ? (
        <ProviderForm
          key={`${editing ? 'edit' : 'create'}:${draft.id || 'new'}`}
          editing={editing}
          initial={draft}
          busy={api.busy !== null}
          onCancel={close}
          onSubmit={submit}
          onTest={(entry) => runTest({ entry }, `test:${entry.id}`)}
          onDiscover={discover}
        />
      ) : null}
    </SettingsSection>
  )
}
