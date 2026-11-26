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
 * deliberately (detail menu or Settings -> General), never as a side effect
 * of adding a provider. Deleting always passes a confirmation dialog because
 * removing the default provider strands every default-following session.
 */
import { useRef, useState, type RefObject } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type {
  ProviderEntry,
  ProviderModelCatalogRequest,
  ProviderModelCatalogResponse,
  ProviderView
} from '../../../shared/contracts'
import type { ProvidersApi } from '../hooks/use-providers'
import { describeOutcome } from '../api/outcome'
import { ProviderForm } from './ProviderForm'
import { ProviderAvatar, ProviderDetail, type TestState } from './ProviderDetail'
import { draftFromView, emptyDraft, type ProviderDraft } from './ProviderDraft'
import { LoadError, SettingsSection, Skeleton } from './SectionShell'
import { useDialogFocus } from '../../../shared/ui/focus'

function ProviderNavItem({ provider, active, isDefault, isActive, onSelect }: {
  provider: ProviderView
  active: boolean
  isDefault: boolean
  isActive: boolean
  onSelect: () => void
}): JSX.Element {
  const { t } = useI18n()
  // One status chip per row, by priority: a disabled provider cannot serve,
  // the default is what new sessions use, and "active" is the runtime state.
  const chip = !provider.enabled
    ? <span className="ark-chip ark-chip--muted">{t('settings.providers.disabled')}</span>
    : isDefault
      ? <span className="ark-chip ark-chip--ok">{t('settings.providers.isDefault')}</span>
      : isActive
        ? <span className="ark-chip ark-chip--live">{t('settings.providers.active')}</span>
        : null
  return (
    <button type="button" className={`ark-provider-nav${active ? ' is-active' : ''}`} onClick={onSelect} aria-current={active ? 'true' : undefined}>
      <ProviderAvatar provider={provider} />
      <span className="ark-provider-nav__copy">
        <strong>{provider.label ?? provider.id}</strong>
        <small>{provider.model}</small>
      </span>
      {chip}
    </button>
  )
}

function DeleteDialog({ provider, isDefault, busy, onCancel, onConfirm, triggerRef }: {
  provider: ProviderView
  isDefault: boolean
  busy: boolean
  onCancel: () => void
  onConfirm: () => void
  triggerRef: RefObject<HTMLButtonElement | null>
}): JSX.Element {
  const { t } = useI18n()
  const dialog = useDialogFocus(true, onCancel, triggerRef)
  return (
    <div ref={dialog.ref} tabIndex={-1} onKeyDown={dialog.onKeyDown} className="ark-importdialog" role="dialog" aria-modal="true" aria-label={t('settings.providers.deleteTitle', { id: provider.id })}>
      <div className="ark-importdialog__panel ark-provider-confirm">
        <h4>{t('settings.providers.deleteTitle', { id: provider.label ?? provider.id })}</h4>
        <p>{isDefault ? t('settings.providers.deleteBodyDefault') : t('settings.providers.deleteBody')}</p>
        <div className="ark-provider-confirm__acts">
          <Button data-autofocus variant="ghost" size="sm" disabled={busy} onClick={onCancel}>{t('common.cancel')}</Button>
          <Button variant="danger" size="sm" disabled={busy} onClick={onConfirm}>{t('common.delete')}</Button>
        </div>
      </div>
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
  const [confirmDelete, setConfirmDelete] = useState(false)
  const editRequestSeq = useRef(0)
  const selectedIdRef = useRef<string | null>(null)
  const deleteTriggerRef = useRef<HTMLButtonElement | null>(null)
  const select = (id: string | null): void => {
    selectedIdRef.current = id
    setSelectedId(id)
  }
  const selected = api.providers.find((row) => row.id === selectedId) ?? api.providers[0] ?? null
  const isDefaultRow = (row: ProviderView): boolean => api.defaultProviderId === row.id || row.is_default

  const close = (): void => {
    editRequestSeq.current += 1
    setDraft(null)
    setEditing(false)
  }

  const submit = async (entry: ProviderEntry): Promise<void> => {
    const outcome = await api.upsert({ ...entry, make_default: false })
    if (outcome.ok) {
      select(entry.id)
      toast.push('success', t('settings.providers.saved', { id: entry.id }))
      close()
      return
    }
    toast.push('error', t('settings.providers.saveFailed'), describeOutcome(outcome))
  }

  const openEdit = async (view: ProviderView): Promise<void> => {
    const request = ++editRequestSeq.current
    select(view.id)
    const outcome = await api.get(view.id)
    if (request !== editRequestSeq.current || view.id !== selectedIdRef.current) return
    if (!outcome.ok) {
      toast.push('error', t('settings.providers.loadFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.id !== view.id) return
    setEditing(true)
    setDraft(draftFromView(outcome.value))
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
    editRequestSeq.current += 1
    const outcome = await api.remove(selected.id)
    setConfirmDelete(false)
    if (outcome.ok) {
      toast.push('success', t('settings.providers.deleted', { id: selected.id }))
      select(null)
      close()
    } else toast.push('error', t('settings.providers.deleteFailed'), describeOutcome(outcome))
  }

  const detail = (): JSX.Element => {
    if (draft) {
      return (
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
      )
    }
    if (!selected) return <EmptyState title={t('settings.providers.detailEmptyTitle')} hint={t('settings.providers.detailEmptyHint')} />
    return (
      <ProviderDetail
        provider={selected}
        busy={api.busy !== null}
        isDefault={isDefaultRow(selected)}
        testState={testState}
        onEdit={() => void openEdit(selected)}
        onTest={() => void runTest({ provider_id: selected.id }, `test:${selected.id}`)}
        onMakeDefault={() => void setDefault()}
        onRemove={(trigger) => { deleteTriggerRef.current = trigger; editRequestSeq.current += 1; setConfirmDelete(true) }}
      />
    )
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.providers.length === 0) return <Skeleton rows={3} />
    if (api.status === 'error') return <LoadError message={api.error ?? t('settings.providers.loadFailed')} onRetry={() => void api.refresh()} />
    if (api.providers.length === 0 && !draft) return <EmptyState title={t('settings.providers.emptyTitle')} hint={t('settings.providers.emptyHint')} />
    return (
      <div className="ark-provider-master">
        <div className="ark-provider-nav-wrap" role="group" aria-label={t('settings.providers.listAria')}>
          {api.providers.map((provider) => (
            <ProviderNavItem
              key={provider.id}
              provider={provider}
              active={!draft && selected?.id === provider.id}
              isDefault={isDefaultRow(provider)}
              isActive={api.activeProviderId === provider.id}
              onSelect={() => { select(provider.id); close() }}
            />
          ))}
        </div>
        <div className="ark-provider-detail-wrap">{detail()}</div>
      </div>
    )
  })()

  return (
    <SettingsSection
      title={t('settings.providers.title')}
      desc={t('settings.providers.desc')}
      actions={
        draft ? null : (
          <Button variant="primary" size="sm" disabled={api.busy !== null} onClick={() => { close(); setEditing(false); setDraft(emptyDraft()) }}>
            {t('settings.providers.add')}
          </Button>
        )
      }
    >
      {api.defaultProviderId !== api.activeProviderId && api.activeProviderId ? (
        <p className="ark-settings__hint ark-settings__hint--warn ark-provider-banner">
          {t('settings.providers.defaultActiveMismatch', { defaultId: api.defaultProviderId ?? t('common.notSet'), activeId: api.activeProviderId })}
        </p>
      ) : null}
      {body}
      {confirmDelete && selected ? (
        <DeleteDialog
          provider={selected}
          isDefault={isDefaultRow(selected)}
          busy={api.busy !== null}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => void remove()}
          triggerRef={deleteTriggerRef}
        />
      ) : null}
    </SettingsSection>
  )
}
