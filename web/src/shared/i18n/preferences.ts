import type { UiFontSize, UiLocale, UiPreferences } from '../contracts/web-only'

export type { UiFontSize, UiLocale, UiPreferences }

export const UI_PREFERENCES_STORAGE_KEY = 'ark.ui.preferences'

export const DEFAULT_UI_PREFERENCES: UiPreferences = {
  locale: 'zh-CN',
  fontSize: 'medium'
}

function isLocale(value: unknown): value is UiLocale {
  return value === 'zh-CN' || value === 'en-US'
}

function isFontSize(value: unknown): value is UiFontSize {
  return value === 'small' || value === 'medium' || value === 'large'
}

export function normalizeUiPreferences(value: unknown): UiPreferences {
  if (!value || typeof value !== 'object') return DEFAULT_UI_PREFERENCES
  const candidate = value as Partial<UiPreferences>
  if (!isLocale(candidate.locale) || !isFontSize(candidate.fontSize)) return DEFAULT_UI_PREFERENCES
  return { locale: candidate.locale, fontSize: candidate.fontSize }
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
