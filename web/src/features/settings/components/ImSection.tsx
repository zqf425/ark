/**
 * IM channel section: list, create/edit form, verify, delete, WeChat QR login,
 * master switch.
 *
 * Everything user-visible is catalog copy; identifiers are wire values. The
 * credentials form never shows stored secrets (the read model does not carry
 * them), and saving with a blank secret keeps the stored one — the runtime's
 * documented merge rule. A confirmed WeChat login creates the channel
 * server-side (disabled); the dialog only surfaces it.
 */
import { useEffect, useMemo, useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import type {
  ApplyResult,
  ImBinding,
  ImChannelKind,
  ImChannelListItem,
  ImChannelView
} from '../../../shared/contracts'
import { listImBindings } from '../api/im-api'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'
import type { SettingsApi } from '../hooks/use-settings'
import type { ProvidersApi } from '../hooks/use-providers'
import { toModelProviderGroups } from '../../../shared/model-provider-menu'
import { useIm } from '../hooks/use-im'
import { describeOutcome, type Outcome } from '../api/outcome'
import { CheckField } from './Field'
import { LoadError, SettingsSection, Skeleton } from './SectionShell'
import { ChannelForm } from './ImChannelForm'
import { WeixinLoginDialog } from './WeixinLoginDialog'
import {
  buildCreateRequest,
  buildUpdateRequest,
  draftFromChannel,
  emptyDraft,
  isUpdateEmpty,
  NO_IM_DEFAULTS,
  type ImChannelDraft,
  type ImFormDefaults
} from './im-form'

/** Mirrors `MAX_IM_CHANNELS`; the runtime is still the authority that rejects. */
const IM_CHANNEL_LIMIT = 20

const KIND_LABEL_KEYS: Record<ImChannelKind, MessageKey> = {
  telegram: 'settings.im.kind.telegram',
  feishu: 'settings.im.kind.feishu',
  weixin: 'settings.im.kind.weixin'
}

const STATE_LABEL_KEYS: Record<string, MessageKey> = {
  disabled: 'settings.im.state.disabled',
  connecting: 'settings.im.state.connecting',
  online: 'settings.im.state.online',
  send_only: 'settings.im.state.sendOnly',
  error: 'settings.im.state.error'
}

function stateLabelKey(state: string): MessageKey {
  return STATE_LABEL_KEYS[state] ?? 'settings.im.state.error'
}

function KindChip({ kind }: { kind: ImChannelKind }): JSX.Element {
  const { t } = useI18n()
  return <span className="ark-chip">{t(KIND_LABEL_KEYS[kind])}</span>
}

function StateChip({ item }: { item: ImChannelListItem }): JSX.Element {
  const { t } = useI18n()
  const state = item.runtime?.state ?? 'disabled'
  const isError = state === 'error'
  return (
    <span className={isError ? 'ark-chip ark-chip--danger' : 'ark-chip ark-chip--muted'}>
      {t(stateLabelKey(state))}
    </span>
  )
}

function BindingsList({ channelId, onOpenThread }: { channelId: string; onOpenThread: (threadId: string) => void }): JSX.Element {
  const { t } = useI18n()
  const [bindings, setBindings] = useState<ImBinding[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    listImBindings(channelId)
      .then((response) => {
        if (!cancelled) setBindings(response.bindings ?? [])
      })
      .catch((thrown) => {
        if (!cancelled) setError(String(thrown))
      })
    return () => {
      cancelled = true
    }
  }, [channelId])
  if (error) return <span className="ark-settings__hint--error">{error}</span>
  if (bindings === null) return <Skeleton rows={1} />
  if (bindings.length === 0) return <span className="ark-settings__hint">{t('settings.im.bindings.empty')}</span>
  return (
    <div className="ark-card__meta">
      {bindings.map((binding) => (
        <span key={`${binding.channel_id}:${binding.chat_id}`}>
          {binding.chat_id} →
          <button
            type="button"
            className="ark-linkbtn"
            onClick={() => onOpenThread(binding.thread_id)}
          >
            {t('settings.im.bindings.openThread')}
          </button>
        </span>
      ))}
    </div>
  )
}

/**
 * The runtime values an un-overridden channel actually uses: the configured
 * default workspace, and the provider the loop is serving (`active`, which is
 * what a turn without an override resolves to) plus that provider's model.
 * They are shown pre-filled so no override field ever renders as a blank box.
 */
function formDefaults(settings: SettingsApi, providers: ProvidersApi): ImFormDefaults {
  const activeId = providers.activeProviderId ?? providers.defaultProviderId
  const active = activeId ? providers.providers.find((row) => row.id === activeId) : undefined
  return {
    workspace: settings.config?.default_workspace ?? NO_IM_DEFAULTS.workspace,
    providerId: active?.id ?? NO_IM_DEFAULTS.providerId,
    model: active?.model ?? NO_IM_DEFAULTS.model
  }
}

export function ImSection({
  settings,
  providers
}: {
  settings: SettingsApi
  providers: ProvidersApi
}): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const api = useIm()
  const { config, saving, save } = settings
  const defaults = formDefaults(settings, providers)
  const selectThread = useCodeSessionStore((state) => state.setActiveThread)
  const setView = useViewStore((state) => state.setView)

  const [draft, setDraft] = useState<ImChannelDraft | null>(null)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const [verified, setVerified] = useState<{ id: string; identity: string } | null>(null)
  const [masterEnabled, setMasterEnabled] = useState(false)
  const [loginOpen, setLoginOpen] = useState(false)

  useEffect(() => {
    if (config) setMasterEnabled(config.im.enabled)
  }, [config])

  const editingChannel = useMemo(
    () => (editingId ? api.channels.find((row) => row.channel.id === editingId)?.channel ?? null : null),
    [api.channels, editingId]
  )

  const persistMaster = async (next: boolean): Promise<void> => {
    setMasterEnabled(next)
    const outcome: Outcome<ApplyResult> = await save({ im: { enabled: next } }, 'im')
    if (!outcome.ok) {
      setMasterEnabled(config?.im?.enabled ?? false)
      toast.push('error', t('settings.im.master.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setMasterEnabled(config?.im?.enabled ?? false)
      toast.push('error', t('settings.im.master.rejected'), outcome.value.reason)
      return
    }
    toast.push('success', t('settings.im.master.succeeded'))
  }

  const toggleChannel = async (channel: ImChannelView, next: boolean): Promise<void> => {
    const outcome = await api.update(channel.id, { enabled: next })
    if (outcome.ok) {
      toast.push('success', t('settings.im.toggled', { title: channel.title }))
      return
    }
    toast.push('error', t('settings.im.toggleFailed'), describeOutcome(outcome))
  }

  const removeChannel = async (channel: ImChannelView): Promise<void> => {
    if (!window.confirm(t('settings.im.deleteConfirm', { title: channel.title }))) return
    const outcome = await api.remove(channel.id)
    if (outcome.ok) {
      if (editingId === channel.id) {
        setDraft(null)
        setEditingId(null)
      }
      toast.push('success', t('settings.im.deleted', { title: channel.title }))
      return
    }
    toast.push('error', t('settings.im.deleteFailed'), describeOutcome(outcome))
  }

  const verifyChannel = async (channel: ImChannelView): Promise<void> => {
    const outcome = await api.verify(channel.id)
    if (outcome.ok) {
      setVerified({ id: channel.id, identity: outcome.value })
      toast.push('success', t('settings.im.verify.succeeded', { identity: outcome.value }))
      return
    }
    setVerified({ id: channel.id, identity: describeOutcome(outcome) })
    toast.push('error', t('settings.im.verify.failed'), describeOutcome(outcome))
  }

  const submit = async (): Promise<void> => {
    if (!draft) return
    setFormError(null)
    if (editingChannel) {
      const request = buildUpdateRequest(editingChannel, draft, defaults)
      if (isUpdateEmpty(request)) {
        setFormError(t('settings.im.nothingChanged'))
        return
      }
      const outcome = await api.update(editingChannel.id, request)
      if (!outcome.ok) {
        setFormError(describeOutcome(outcome))
        toast.push('error', t('settings.im.saveFailed'), describeOutcome(outcome))
        return
      }
      toast.push('success', t('settings.im.updated', { title: outcome.value.title }))
      setDraft(null)
      setEditingId(null)
      return
    }
    const outcome = await api.create(buildCreateRequest(draft, defaults))
    if (!outcome.ok) {
      setFormError(describeOutcome(outcome))
      toast.push('error', t('settings.im.saveFailed'), describeOutcome(outcome))
      return
    }
    toast.push('success', t('settings.im.created', { title: outcome.value.title }))
    setDraft(null)
    setEditingId(null)
  }

  const openThread = (threadId: string): void => {
    selectThread(threadId)
    setView('code')
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.channels.length === 0) {
      return <Skeleton rows={2} />
    }
    if (api.status === 'error') {
      return (
        <LoadError
          message={api.error ?? t('settings.im.loadFailed')}
          onRetry={() => void api.refresh()}
        />
      )
    }
    if (api.channels.length === 0) {
      return <EmptyState title={t('settings.im.emptyTitle')} hint={t('settings.im.emptyHint')} />
    }
    return (
      <div className="ark-settings__list">
        {api.channels.map((item) => (
          <div className="ark-card" key={item.channel.id}>
            <div className="ark-card__head">
              <span className="ark-card__name">{item.channel.title || t('canvas.untitled')}</span>
              <KindChip kind={item.channel.kind} />
              <StateChip item={item} />
              {item.channel.enabled ? null : (
                <span className="ark-chip ark-chip--muted">{t('settings.im.enabledOff')}</span>
              )}
              <div className="ark-card__actions">
                <Button size="sm" disabled={api.busy !== null} onClick={() => void verifyChannel(item.channel)}>
                  {t('settings.im.verify')}
                </Button>
                <Button
                  size="sm"
                  disabled={api.busy !== null}
                  onClick={() => {
                    setFormError(null)
                    setEditingId(item.channel.id)
                    setDraft(draftFromChannel(item.channel, defaults))
                  }}
                >
                  {t('common.edit')}
                </Button>
                <Button variant="danger" size="sm" disabled={api.busy !== null} onClick={() => void removeChannel(item.channel)}>
                  {t('common.delete')}
                </Button>
              </div>
            </div>
            <div className="ark-card__meta">
              <span>
                {t('settings.im.bindings.label')}:
                <BindingsList channelId={item.channel.id} onOpenThread={openThread} />
              </span>
            </div>
            {item.runtime?.last_error ? (
              <div className="ark-card__meta">
                <span className="ark-settings__hint--error">{item.runtime.last_error}</span>
              </div>
            ) : null}
            {verified?.id === item.channel.id ? (
              <div className="ark-card__meta">
                <span>
                  {t('settings.im.verify.identity')}: {verified.identity}
                </span>
              </div>
            ) : null}
            <div className="ark-card__foot">
              <CheckField
                label={t('settings.im.enabledLabel')}
                checked={item.channel.enabled}
                disabled={api.busy !== null}
                onChange={(checked) => void toggleChannel(item.channel, checked)}
              />
              <span className="ark-settings__hint">{t('settings.im.toggleHint')}</span>
            </div>
          </div>
        ))}
      </div>
    )
  })()

  return (
    <SettingsSection
      title={t('settings.im.title')}
      desc={t('settings.im.desc')}
      actions={
        <div className="ark-card__actions">
          <Button size="sm" disabled={api.busy !== null} onClick={() => setLoginOpen(true)}>
            {t('settings.im.login.title')}
          </Button>
          <Button
            size="sm"
            disabled={api.channels.length >= IM_CHANNEL_LIMIT}
            onClick={() => {
              setFormError(null)
              setEditingId(null)
              setDraft(emptyDraft(defaults))
            }}
          >
            {t('settings.im.create')}
          </Button>
        </div>
      }
    >
      <div className="ark-card">
        <div className="ark-card__foot">
          <CheckField
            label={t('settings.im.master.label')}
            checked={masterEnabled}
            disabled={saving != null}
            onChange={(checked) => void persistMaster(checked)}
          />
          <span className="ark-settings__hint">{t('settings.im.master.hint')}</span>
        </div>
      </div>
      {body}
      {draft ? (
        <ChannelForm
          draft={draft}
          groups={toModelProviderGroups(providers.providers)}
          defaults={defaults}
          onDraft={setDraft}
          editing={editingId !== null}
          busy={api.busy !== null}
          formError={formError}
          onSubmit={() => void submit()}
          onCancel={() => {
            setDraft(null)
            setEditingId(null)
          }}
          onSaveMaster={(next) => void persistMaster(next)}
        />
      ) : null}
      {loginOpen ? <WeixinLoginDialog onClose={() => setLoginOpen(false)} /> : null}
    </SettingsSection>
  )
}

