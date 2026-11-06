/**
 * IM channel create/edit form (display only).
 *
 * Mapping lives in `im-form.ts`; policy semantics stay in `ark-im`. Enabling a
 * channel also turns the master switch on, so the two switches cannot disagree
 * after the next reload.
 */
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { FeishuDomain, ImChannelKind } from '../../../shared/contracts'
import type { MessageKey } from '../../../shared/i18n'
import { CheckField, Field, SelectField, TextField } from './Field'
import { Notice } from './SectionShell'
import { IM_AUTONOMY_VALUES, IM_KINDS, type ImChannelDraft, type ImFormDefaults } from './im-form'
import {
  ModelProviderMenu,
  type ModelProviderGroup
} from '../../../shared/model-provider-menu'

const KIND_LABEL_KEYS: Record<ImChannelKind, MessageKey> = {
  telegram: 'settings.im.kind.telegram',
  feishu: 'settings.im.kind.feishu',
  weixin: 'settings.im.kind.weixin'
}

export function CredentialsFields({
  draft,
  onDraft
}: {
  draft: ImChannelDraft
  onDraft: (next: ImChannelDraft) => void
}): JSX.Element {
  const { t } = useI18n()
  if (draft.kind === 'telegram') {
    return (
      <>
        <TextField
          id="im-tg-token"
          label={t('settings.im.credentials.telegram.botToken')}
          value={draft.credentials.telegram.botToken}
          onChange={(value) =>
            onDraft({
              ...draft,
              credentials: { ...draft.credentials, telegram: { ...draft.credentials.telegram, botToken: value } }
            })
          }
        />
        <TextField
          id="im-tg-username"
          label={t('settings.im.credentials.telegram.botUsername')}
          value={draft.credentials.telegram.botUsername}
          onChange={(value) =>
            onDraft({
              ...draft,
              credentials: { ...draft.credentials, telegram: { ...draft.credentials.telegram, botUsername: value } }
            })
          }
        />
      </>
    )
  }
  if (draft.kind === 'feishu') {
    return (
      <>
        <TextField
          id="im-fs-appid"
          label={t('settings.im.credentials.feishu.appId')}
          value={draft.credentials.feishu.appId}
          onChange={(value) =>
            onDraft({
              ...draft,
              credentials: { ...draft.credentials, feishu: { ...draft.credentials.feishu, appId: value } }
            })
          }
        />
        <TextField
          id="im-fs-secret"
          label={t('settings.im.credentials.feishu.appSecret')}
          value={draft.credentials.feishu.appSecret}
          onChange={(value) =>
            onDraft({
              ...draft,
              credentials: { ...draft.credentials, feishu: { ...draft.credentials.feishu, appSecret: value } }
            })
          }
        />
        <SelectField<FeishuDomain>
          id="im-fs-domain"
          label={t('settings.im.credentials.feishu.domain')}
          value={draft.credentials.feishu.domain}
          options={['feishu', 'lark']}
          labels={{
            feishu: t('settings.im.credentials.feishu.domainFeishu'),
            lark: t('settings.im.credentials.feishu.domainLark')
          }}
          onChange={(value) =>
            onDraft({
              ...draft,
              credentials: { ...draft.credentials, feishu: { ...draft.credentials.feishu, domain: value } }
            })
          }
        />
      </>
    )
  }
  return (
    <>
      <TextField
        id="im-wx-token"
        label={t('settings.im.credentials.weixin.botToken')}
        value={draft.credentials.weixin.botToken}
        onChange={(value) =>
          onDraft({
            ...draft,
            credentials: { ...draft.credentials, weixin: { ...draft.credentials.weixin, botToken: value } }
          })
        }
      />
      <TextField
        id="im-wx-baseurl"
        label={t('settings.im.credentials.weixin.baseUrl')}
        value={draft.credentials.weixin.baseUrl}
        onChange={(value) =>
          onDraft({
            ...draft,
            credentials: { ...draft.credentials, weixin: { ...draft.credentials.weixin, baseUrl: value } }
          })
        }
      />
      <TextField
        id="im-wx-owner"
        label={t('settings.im.credentials.weixin.ownerUserId')}
        value={draft.credentials.weixin.ownerUserId}
        onChange={(value) =>
          onDraft({
            ...draft,
            credentials: { ...draft.credentials, weixin: { ...draft.credentials.weixin, ownerUserId: value } }
          })
        }
      />
    </>
  )
}

export function ChannelForm({
  draft,
  onDraft,
  editing,
  busy,
  formError,
  onSubmit,
  onCancel,
  onSaveMaster,
  groups,
  defaults
}: {
  draft: ImChannelDraft
  onDraft: (next: ImChannelDraft) => void
  editing: boolean
  busy: boolean
  formError: string | null
  onSubmit: () => void
  onCancel: () => void
  onSaveMaster: (next: boolean) => void
  /** Provider -> model groups for the single two-level menu. */
  groups: readonly ModelProviderGroup[]
  /** Runtime defaults, restored by the "follow default" entry. */
  defaults: ImFormDefaults
}): JSX.Element {
  const { t } = useI18n()
  return (
    <form
      className="ark-card"
      onSubmit={(event) => {
        event.preventDefault()
        onSubmit()
      }}
    >
      <div className="ark-card__head">
        <span className="ark-card__name">
          {t(editing ? 'settings.im.form.editTitle' : 'settings.im.form.createTitle')}
        </span>
      </div>
      <div className="ark-settings__grid">
        <SelectField<ImChannelKind>
          id="im-form-kind"
          label={t('settings.im.form.kind')}
          value={draft.kind}
          disabled={editing}
          options={IM_KINDS}
          labels={Object.fromEntries(
            IM_KINDS.map((kind) => [kind, t(KIND_LABEL_KEYS[kind])])
          ) as Partial<Record<ImChannelKind, string>>}
          onChange={(value) => onDraft({ ...draft, kind: value })}
        />
        <TextField
          id="im-form-title"
          label={t('settings.im.form.title')}
          value={draft.title}
          onChange={(value) => onDraft({ ...draft, title: value })}
        />
        <CredentialsFields draft={draft} onDraft={onDraft} />
        <Field
          label={t('settings.im.form.allowedChatIds')}
          hint={t('settings.im.form.allowedChatIdsHint')}
        >
          <textarea
            className="ark-settings__textarea"
            rows={3}
            value={draft.allowedChatIds}
            onChange={(event) => onDraft({ ...draft, allowedChatIds: event.target.value })}
          />
        </Field>
        <CheckField
          label={t('settings.im.form.requireMention')}
          checked={draft.requireMention}
          onChange={(checked) => onDraft({ ...draft, requireMention: checked })}
        />
        <TextField
          id="im-form-workspace"
          label={t('settings.im.form.workspace')}
          hint={t('settings.im.form.workspaceHint')}
          value={draft.workspace}
          onChange={(value) => onDraft({ ...draft, workspace: value })}
        />
        <Field label={t('settings.im.form.model')} htmlFor="im-form-model" hint={t('settings.im.form.modelHint')}>
          <ModelProviderMenu
            id="im-form-model"
            groups={groups}
            value={{ providerId: draft.providerId, model: draft.model }}
            disabled={busy}
            ariaLabel={t('settings.im.form.model')}
            onSelect={(selection) =>
              onDraft({ ...draft, providerId: selection.providerId, model: selection.model })
            }
            clearSelection={{
              label: t('modelProviderMenu.followDefault'),
              onClear: () =>
                onDraft({ ...draft, providerId: defaults.providerId, model: defaults.model })
            }}
          />
        </Field>
        <SelectField<(typeof IM_AUTONOMY_VALUES)[number]>
          id="im-form-autonomy"
          label={t('settings.im.form.autonomy')}
          value={draft.autonomy as (typeof IM_AUTONOMY_VALUES)[number]}
          options={IM_AUTONOMY_VALUES}
          onChange={(value) => onDraft({ ...draft, autonomy: value })}
        />
        <CheckField
          label={t('settings.im.form.enabled')}
          checked={draft.enabled}
          onChange={(checked) => {
            // Enabling a channel with the master switch off would connect
            // nothing; turning the master on here keeps the two switches from
            // disagreeing after the next reload.
            if (checked) onSaveMaster(true)
            onDraft({ ...draft, enabled: checked })
          }}
        />
      </div>
      {formError ? (
        <Notice tone="error">{formError}</Notice>
      ) : (
        <Notice>{t('settings.im.form.secretsHint')}</Notice>
      )}
      <div className="ark-card__actions">
        <Button type="submit" size="sm" disabled={busy}>
          {t('common.save')}
        </Button>
        <Button type="button" size="sm" onClick={onCancel}>
          {t('common.cancel')}
        </Button>
      </div>
    </form>
  )
}
