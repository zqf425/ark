import { describe, expect, it } from 'vitest'
import type { ImChannelView } from '../../../shared/contracts'
import {
  buildCreateRequest,
  buildUpdateRequest,
  draftFromChannel,
  emptyDraft,
  isUpdateEmpty,
  parseAllowedChatIds
} from './im-form'

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
})
