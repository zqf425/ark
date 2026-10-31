import { describe, expect, it } from 'vitest'
import { MESSAGES, translate } from '../../../shared/i18n/messages'

const REWIND_KEYS = Object.keys(MESSAGES).filter((key) => key.startsWith('session.rewind.'))

describe('rewind i18n keys', () => {
  it('中英文资源都提供 rewind 消息键', () => {
    expect(REWIND_KEYS.length).toBeGreaterThan(0)
    for (const key of REWIND_KEYS) {
      expect(translate('zh-CN', key as never), key).not.toBe('')
      expect(translate('en-US', key as never), key).not.toBe('')
    }
  })

  it('包含入口与确认文案', () => {
    expect(REWIND_KEYS).toContain('session.rewind.entry')
    expect(REWIND_KEYS).toContain('session.rewind.confirmTitle')
    expect(REWIND_KEYS).toContain('session.rewind.confirmBody')
    expect(REWIND_KEYS).toContain('session.rewind.confirmAction')
    expect(REWIND_KEYS).toContain('session.rewind.cancel')
    expect(REWIND_KEYS).toContain('session.rewind.failed')
  })
})
