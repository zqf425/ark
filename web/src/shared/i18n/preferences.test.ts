import { describe, expect, it } from 'vitest'
import { DEFAULT_UI_PREFERENCES, normalizeUiPreferences } from './preferences'

describe('UI preferences', () => {
  it('defaults to Chinese, a medium font, and light theme while rejecting unsupported persisted values', () => {
    expect(DEFAULT_UI_PREFERENCES).toEqual({ locale: 'zh-CN', fontSize: 'medium', theme: 'light' })
    expect(normalizeUiPreferences({ locale: 'en-US', fontSize: 'large', theme: 'dark' })).toEqual({ locale: 'en-US', fontSize: 'large', theme: 'dark' })
    expect(normalizeUiPreferences({ locale: 'en-US', fontSize: 'large' })).toEqual({ locale: 'en-US', fontSize: 'large', theme: 'light' })
    expect(normalizeUiPreferences({ locale: 'fr-FR', fontSize: 'huge' })).toEqual(DEFAULT_UI_PREFERENCES)
  })
})
