import { describe, expect, it } from 'vitest'
import type { ImChannelView } from '../../../shared/contracts'
import {
  buildCreateRequest,
  buildUpdateRequest,
  draftFromChannel,
  emptyDraft,
  isUpdateEmpty,
  parseAllowedChatIds,
  type ImFormDefaults
} from './im-form'

const DEFAULTS: ImFormDefaults = {
  workspace: '/Users/me/work',
  providerId: 'openai',
  model: 'gpt-5'
}

function channel(overrides: Partial<ImChannelView> = {}): ImChannelView {
  return {
    id: 'im_1',
    kind: 'telegram',
    title: 'bot',
    enabled: false,
    credentials: { kind: 'telegram', bot_username: 'ark_bot', has_bot_token: true },
    policy: { allowed_chat_ids: ['100'], require_mention: true },
    workspace: undefined,
    model: undefined,
    provider_id: undefined,
    autonomy: 'unattended',
    created_at: '2026-11-01T00:00:00Z',
    updated_at: '2026-11-01T00:00:00Z',
    ...overrides
  }
}

describe('im-form', () => {
  it('parses the whitelist textarea with trim and dedupe', () => {
    expect(parseAllowedChatIds(' 100 \n200\n100\n\n')).toEqual(['100', '200'])
    expect(parseAllowedChatIds('   ')).toEqual([])
  })

  it('builds a create request with normalized policy', () => {
    const draft = emptyDraft()
    draft.kind = 'feishu'
    draft.title = ' my bot '
    draft.enabled = true
    draft.credentials.feishu = { appId: 'a', appSecret: 's', domain: 'lark' }
    draft.allowedChatIds = ' 42 \n42\n7'
    const request = buildCreateRequest(draft)
    expect(request.kind).toBe('feishu')
    expect(request.title).toBe('my bot')
    expect(request.credentials).toEqual({ kind: 'feishu', app_id: 'a', app_secret: 's', domain: 'lark' })
    expect(request.policy?.allowed_chat_ids).toEqual(['42', '7'])
    expect(request.autonomy).toBe('unattended')
  })

  it('omits fields the draft left unchanged so a patch keeps stored values', () => {
    const draft = draftFromChannel(channel())
    draft.title = 'renamed'
    const request = buildUpdateRequest(channel(), draft)
    expect(request.title).toBe('renamed')
    // Untouched credentials (blank secret, unchanged non-secret fields) are
    // omitted entirely; a blank secret on a changed set keeps the stored one.
    expect(request.credentials).toBeUndefined()
    expect(request.policy).toBeUndefined()
    expect(request.enabled).toBeUndefined()
    expect(request.workspace).toBeUndefined()
  })

  it('detects an empty update', () => {
    const draft = draftFromChannel(channel())
    expect(isUpdateEmpty(buildUpdateRequest(channel(), draft))).toBe(true)
    draft.title = 'changed'
    expect(isUpdateEmpty(buildUpdateRequest(channel(), draft))).toBe(false)
  })

  it('pre-fills the override fields with the runtime defaults', () => {
    const blank = emptyDraft(DEFAULTS)
    expect(blank.workspace).toBe('/Users/me/work')
    expect(blank.providerId).toBe('openai')
    expect(blank.model).toBe('gpt-5')

    // A channel without overrides shows the same defaults, never a blank box.
    const stored = draftFromChannel(channel(), DEFAULTS)
    expect(stored.workspace).toBe('/Users/me/work')
    expect(stored.providerId).toBe('openai')
    expect(stored.model).toBe('gpt-5')

    // A stored override wins over the default.
    const overridden = draftFromChannel(
      channel({ workspace: '/srv/bot', provider_id: 'local', model: 'qwen3' }),
      DEFAULTS
    )
    expect(overridden.workspace).toBe('/srv/bot')
    expect(overridden.providerId).toBe('local')
    expect(overridden.model).toBe('qwen3')
  })

  it('treats a pre-filled default as "follow the default" on create', () => {
    const request = buildCreateRequest(emptyDraft(DEFAULTS), DEFAULTS)
    expect(request.workspace).toBeUndefined()
    expect(request.provider_id).toBeUndefined()
    expect(request.model).toBeUndefined()

    // Typing something else is a real override.
    const overridden = buildCreateRequest(
      { ...emptyDraft(DEFAULTS), workspace: ' /srv/bot ', model: 'qwen3' },
      DEFAULTS
    )
    expect(overridden.workspace).toBe('/srv/bot')
    expect(overridden.model).toBe('qwen3')
    expect(overridden.provider_id).toBeUndefined()
  })

  it('keeps an untouched edit empty and clears a dropped override', () => {
    const stored = channel({ workspace: '/srv/bot', provider_id: 'local', model: 'qwen3' })
    // Opened from the list with defaults filled in: nothing changed yet.
    expect(isUpdateEmpty(buildUpdateRequest(stored, draftFromChannel(stored, DEFAULTS), DEFAULTS))).toBe(true)

    // Replacing the override with the pre-filled default clears it on the wire.
    const reverted = buildUpdateRequest(
      stored,
      { ...draftFromChannel(stored, DEFAULTS), workspace: DEFAULTS.workspace },
      DEFAULTS
    )
    expect(reverted.workspace).toBe('')
    expect(reverted.model).toBeUndefined()
  })
})
