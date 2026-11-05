/**
 * IM channel form mapping (display only).
 *
 * Policy semantics (whitelist meaning, credential merge) live in `ark-im`;
 * this module only shapes the wire payloads and reports the secret-blank rule
 * to the user. Secrets are never displayed back (`ImCredentialsView` carries
 * only `has_*` flags), so an untouched secret field is sent empty and the
 * runtime keeps the stored value.
 */
import type {
  CreateImChannelRequest,
  FeishuDomain,
  ImChannelKind,
  ImChannelView,
  ImCredentials,
  UpdateImChannelRequest
} from '../../../shared/contracts'

export type ImCredentialsDraft = {
  telegram: { botToken: string; botUsername: string }
  feishu: { appId: string; appSecret: string; domain: FeishuDomain }
  weixin: { botToken: string; baseUrl: string; ownerUserId: string }
}

export type ImChannelDraft = {
  kind: ImChannelKind
  title: string
  enabled: boolean
  credentials: ImCredentialsDraft
  /** One chat id per line; whitespace and duplicates are normalized away. */
  allowedChatIds: string
  requireMention: boolean
  workspace: string
  model: string
  providerId: string
  autonomy: string
}

export const IM_KINDS: readonly ImChannelKind[] = ['telegram', 'feishu', 'weixin']

export const IM_AUTONOMY_VALUES = ['strict', 'workspace_write', 'unattended'] as const

/**
 * Runtime values the three override fields fall back to.
 *
 * The form shows them filled in, so "empty = default" never renders as a blank
 * box; on save a value equal to its default is sent as "no override", which
 * keeps the channel following later default changes instead of pinning today's
 * value. `NO_IM_DEFAULTS` is what a caller without runtime config sees.
 */
export type ImFormDefaults = {
  workspace: string
  providerId: string
  model: string
}

export const NO_IM_DEFAULTS: ImFormDefaults = { workspace: '', providerId: '', model: '' }

export function emptyCredentialsDraft(): ImCredentialsDraft {
  return {
    telegram: { botToken: '', botUsername: '' },
    feishu: { appId: '', appSecret: '', domain: 'feishu' },
    weixin: { botToken: '', baseUrl: '', ownerUserId: '' }
  }
}

export function emptyDraft(defaults: ImFormDefaults = NO_IM_DEFAULTS): ImChannelDraft {
  return {
    kind: 'telegram',
    title: '',
    enabled: false,
    credentials: emptyCredentialsDraft(),
    allowedChatIds: '',
    requireMention: true,
    workspace: defaults.workspace,
    model: defaults.model,
    providerId: defaults.providerId,
    autonomy: 'unattended'
  }
}

/**
 * Secrets stay blank: the read model never carries them back. An override the
 * channel does not carry shows the runtime default instead of an empty box.
 */
export function draftFromChannel(
  channel: ImChannelView,
  defaults: ImFormDefaults = NO_IM_DEFAULTS
): ImChannelDraft {
  const draft = emptyDraft(defaults)
  draft.kind = channel.kind
  draft.title = channel.title
  draft.enabled = channel.enabled
  draft.allowedChatIds = channel.policy.allowed_chat_ids.join('\n')
  draft.requireMention = channel.policy.require_mention
  draft.workspace = channel.workspace ?? defaults.workspace
  draft.model = channel.model ?? defaults.model
  draft.providerId = channel.provider_id ?? defaults.providerId
  draft.autonomy = channel.autonomy ?? 'unattended'
  const credentials = channel.credentials
  if (credentials) {
    if (credentials.kind === 'telegram') {
      draft.credentials.telegram.botUsername = credentials.bot_username ?? ''
    }
    if (credentials.kind === 'feishu') {
      draft.credentials.feishu.appId = credentials.app_id
      draft.credentials.feishu.domain = credentials.domain
    }
    if (credentials.kind === 'weixin') {
      draft.credentials.weixin.baseUrl = credentials.base_url ?? ''
      draft.credentials.weixin.ownerUserId = credentials.owner_user_id
    }
  }
  return draft
}

/** Split the textarea into normalized chat ids (trim, drop empties, dedupe). */
export function parseAllowedChatIds(raw: string): string[] {
  return [...new Set(
    raw
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line.length > 0)
  )]
}

function credentialsFromDraft(draft: ImChannelDraft): ImCredentials {
  switch (draft.kind) {
    case 'telegram': {
      const bot_token = draft.credentials.telegram.botToken.trim()
      const bot_username = draft.credentials.telegram.botUsername.trim()
      return {
        kind: 'telegram',
        bot_token,
        ...(bot_username ? { bot_username } : {})
      }
    }
    case 'feishu':
      return {
        kind: 'feishu',
        app_id: draft.credentials.feishu.appId.trim(),
        app_secret: draft.credentials.feishu.appSecret.trim(),
        domain: draft.credentials.feishu.domain
      }
    case 'weixin':
      return {
        kind: 'weixin',
        bot_token: draft.credentials.weixin.botToken.trim(),
        ...(draft.credentials.weixin.baseUrl.trim()
          ? { base_url: draft.credentials.weixin.baseUrl.trim() }
          : {}),
        owner_user_id: draft.credentials.weixin.ownerUserId.trim()
      }
  }
}

/**
 * The override a draft carries, or `null` when it should follow the default.
 *
 * Blank and "exactly the default" both mean follow: the field is displayed
 * pre-filled, so echoing the default back must not become a pinned override.
 */
function overrideOf(value: string, fallback: string): string | null {
  const trimmed = value.trim()
  if (trimmed === '' || trimmed === fallback.trim()) return null
  return trimmed
}

/** Create: following the default is simply absent from the payload. */
function createOverride(value: string, fallback: string): string | undefined {
  return overrideOf(value, fallback) ?? undefined
}

/**
 * Update: following the default clears a stored override (empty string on the
 * wire); when nothing is stored either, the field is omitted as unchanged.
 */
function updateOverride(
  value: string,
  fallback: string,
  stored: string | null
): string | undefined {
  const desired = overrideOf(value, fallback)
  if (desired === stored) return undefined
  return desired ?? ''
}

function storedOverride(value: string | undefined, fallback: string): string | null {
  // A stored value that already equals the default is not an override: the
  // form shows the same value either way, so treating it as one would report a
  // spurious change the moment the editor opens.
  return overrideOf(value ?? '', fallback)
}

export function buildCreateRequest(
  draft: ImChannelDraft,
  defaults: ImFormDefaults = NO_IM_DEFAULTS
): CreateImChannelRequest {
  return {
    kind: draft.kind,
    title: draft.title.trim() || undefined,
    enabled: draft.enabled,
    credentials: credentialsFromDraft(draft),
    policy: {
      allowed_chat_ids: parseAllowedChatIds(draft.allowedChatIds),
      require_mention: draft.requireMention
    },
    workspace: createOverride(draft.workspace, defaults.workspace),
    model: createOverride(draft.model, defaults.model),
    provider_id: createOverride(draft.providerId, defaults.providerId),
    autonomy: draft.autonomy.trim() || undefined
  }
}

/**
 * Whether the draft's credentials differ from the stored view: a non-blank
 * secret is a change, a blank secret keeps the stored one, and non-secret
 * fields compare directly. Unchanged credentials are omitted so an otherwise
 * empty edit reads as "nothing to save".
 */
function credentialsChanged(current: ImChannelView, draft: ImChannelDraft): boolean {
  const stored = current.credentials
  const incoming = credentialsFromDraft(draft)
  if (!stored || stored.kind !== incoming.kind) return true
  if (incoming.kind === 'telegram' && stored.kind === 'telegram') {
    return (
      incoming.bot_token.trim() !== '' ||
      (incoming.bot_username ?? '') !== (stored.bot_username ?? '')
    )
  }
  if (incoming.kind === 'feishu' && stored.kind === 'feishu') {
    return (
      incoming.app_secret !== '' ||
      incoming.app_id !== stored.app_id ||
      incoming.domain !== stored.domain
    )
  }
  if (incoming.kind === 'weixin' && stored.kind === 'weixin') {
    return (
      incoming.bot_token !== '' ||
      (incoming.base_url ?? '') !== (stored.base_url ?? '') ||
      incoming.owner_user_id !== stored.owner_user_id
    )
  }
  return true
}

export function buildUpdateRequest(
  current: ImChannelView,
  draft: ImChannelDraft,
  defaults: ImFormDefaults = NO_IM_DEFAULTS
): UpdateImChannelRequest {
  const currentChatIds = current.policy.allowed_chat_ids.join('\n')
  const request: UpdateImChannelRequest = {
    title: draft.title.trim() === current.title ? undefined : draft.title.trim(),
    enabled: draft.enabled === current.enabled ? undefined : draft.enabled,
    credentials: credentialsChanged(current, draft) ? credentialsFromDraft(draft) : undefined,
    policy:
      currentChatIds === draft.allowedChatIds && current.policy.require_mention === draft.requireMention
        ? undefined
        : {
            allowed_chat_ids: parseAllowedChatIds(draft.allowedChatIds),
            require_mention: draft.requireMention
          },
    workspace: updateOverride(draft.workspace, defaults.workspace, storedOverride(current.workspace, defaults.workspace)),
    model: updateOverride(draft.model, defaults.model, storedOverride(current.model, defaults.model)),
    provider_id: updateOverride(
      draft.providerId,
      defaults.providerId,
      storedOverride(current.provider_id, defaults.providerId)
    ),
    autonomy:
      draft.autonomy.trim() === (current.autonomy ?? '').trim()
        ? undefined
        : draft.autonomy.trim()
  }
  return request
}

/** True when nothing in the draft differs from the stored channel. */
export function isUpdateEmpty(request: UpdateImChannelRequest): boolean {
  return Object.values(request).every((value) => value === undefined)
}
