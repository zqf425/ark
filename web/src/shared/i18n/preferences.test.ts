import { describe, expect, it } from 'vitest'
import { DEFAULT_UI_PREFERENCES, normalizeUiPreferences } from './preferences'

describe('UI preferences', () => {
  it('defaults to Chinese with the medium font size and rejects unsupported persisted values', () => {
    expect(DEFAULT_UI_PREFERENCES).toEqual({ locale: 'zh-CN', fontSize: 'medium' })
    expect(normalizeUiPreferences({ locale: 'en-US', fontSize: 'large' })).toEqual({ locale: 'en-US', fontSize: 'large' })
    expect(normalizeUiPreferences({ locale: 'fr-FR', fontSize: 'huge' })).toEqual(DEFAULT_UI_PREFERENCES)
  })
})
