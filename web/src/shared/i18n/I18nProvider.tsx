import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { translate, type MessageKey } from './messages'
import { DEFAULT_UI_PREFERENCES, readUiPreferences, type UiFontSize, type UiLocale, type UiPreferences, type UiTheme, UI_PREFERENCES_STORAGE_KEY } from './preferences'

type I18nContextValue = UiPreferences & {
  setLocale: (locale: UiLocale) => void
  setFontSize: (fontSize: UiFontSize) => void
  setTheme: (theme: UiTheme) => void
  t: (key: MessageKey) => string
}

const I18nContext = createContext<I18nContextValue | null>(null)

function initialPreferences(): UiPreferences {
  if (typeof window === 'undefined') return DEFAULT_UI_PREFERENCES
  return readUiPreferences(window.localStorage)
}

export function I18nProvider({ children }: { children: ReactNode }): JSX.Element {
  const [preferences, setPreferences] = useState<UiPreferences>(initialPreferences)

  useEffect(() => {
    document.documentElement.lang = preferences.locale
    document.documentElement.dataset.arkFontSize = preferences.fontSize
    document.documentElement.dataset.arkTheme = preferences.theme
    window.localStorage.setItem(UI_PREFERENCES_STORAGE_KEY, JSON.stringify(preferences))
  }, [preferences])

  const value = useMemo<I18nContextValue>(() => ({
    ...preferences,
    setLocale: (locale) => setPreferences((current) => ({ ...current, locale })),
    setFontSize: (fontSize) => setPreferences((current) => ({ ...current, fontSize })),
    setTheme: (theme) => setPreferences((current) => ({ ...current, theme })),
    t: (key) => translate(preferences.locale, key)
  }), [preferences])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext)
  if (!context) throw new Error('useI18n 必须在 I18nProvider 内使用')
  return context
}
