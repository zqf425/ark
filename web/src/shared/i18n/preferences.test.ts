import { describe, expect, it } from 'vitest'
import { DEFAULT_UI_PREFERENCES, normalizeUiPreferences, resolveTheme } from './preferences'

describe('UI preferences', () => {
  it('defaults to Chinese, a medium font, and the system theme while rejecting unsupported persisted values', () => {
    expect(DEFAULT_UI_PREFERENCES).toEqual({ locale: 'zh-CN', fontSize: 'medium', theme: 'system' })
    expect(normalizeUiPreferences({ locale: 'en-US', fontSize: 'large', theme: 'dark' })).toEqual({ locale: 'en-US', fontSize: 'large', theme: 'dark' })
    expect(normalizeUiPreferences({ locale: 'en-US', fontSize: 'large' })).toEqual({ locale: 'en-US', fontSize: 'large', theme: 'system' })
    expect(normalizeUiPreferences({ locale: 'fr-FR', fontSize: 'huge' })).toEqual(DEFAULT_UI_PREFERENCES)
  })

  it('resolves the system preference from the active OS color scheme', () => {
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
  })
})
