import type { UiFontSize, UiLocale, UiPreferences, UiTheme } from '../contracts/web-only'

export type { UiFontSize, UiLocale, UiPreferences, UiTheme }

export const UI_PREFERENCES_STORAGE_KEY = 'ark.ui.preferences'

export const DEFAULT_UI_PREFERENCES: UiPreferences = {
  locale: 'zh-CN',
  fontSize: 'medium',
  theme: 'system'
}

function isLocale(value: unknown): value is UiLocale {
  return value === 'zh-CN' || value === 'en-US'
}

function isFontSize(value: unknown): value is UiFontSize {
  return value === 'small' || value === 'medium' || value === 'large'
}

function isTheme(value: unknown): value is UiTheme {
  return value === 'light' || value === 'dark' || value === 'system'
}

export function resolveTheme(theme: UiTheme, systemPrefersDark: boolean): 'light' | 'dark' {
  if (theme === 'system') return systemPrefersDark ? 'dark' : 'light'
  return theme
}

export function normalizeUiPreferences(value: unknown): UiPreferences {
  if (!value || typeof value !== 'object') return DEFAULT_UI_PREFERENCES
  const candidate = value as Partial<UiPreferences>
  if (!isLocale(candidate.locale) || !isFontSize(candidate.fontSize)) return DEFAULT_UI_PREFERENCES
  return { locale: candidate.locale, fontSize: candidate.fontSize, theme: isTheme(candidate.theme) ? candidate.theme : DEFAULT_UI_PREFERENCES.theme }
}

export function readUiPreferences(storage: Pick<Storage, 'getItem'>): UiPreferences {
  const raw = storage.getItem(UI_PREFERENCES_STORAGE_KEY)
  if (!raw) return DEFAULT_UI_PREFERENCES
  try {
    return normalizeUiPreferences(JSON.parse(raw))
  } catch {
    return DEFAULT_UI_PREFERENCES
  }
}
