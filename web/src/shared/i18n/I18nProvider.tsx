import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { setActiveLocale } from './current'
import { translate, type MessageKey, type MessageParams } from './messages'
import { DEFAULT_UI_PREFERENCES, readUiPreferences, resolveTheme, type UiFontSize, type UiLocale, type UiPreferences, type UiTheme, UI_PREFERENCES_STORAGE_KEY } from './preferences'

type I18nContextValue = UiPreferences & {
  setLocale: (locale: UiLocale) => void
  setFontSize: (fontSize: UiFontSize) => void
  setTheme: (theme: UiTheme) => void
  t: (key: MessageKey, params?: MessageParams) => string
}

const I18nContext = createContext<I18nContextValue | null>(null)

function initialPreferences(): UiPreferences {
  if (typeof window === 'undefined') return DEFAULT_UI_PREFERENCES
  return readUiPreferences(window.localStorage)
}

function initialSystemPrefersDark(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches
}

export function I18nProvider({ children }: { children: ReactNode }): JSX.Element {
  const [preferences, setPreferences] = useState<UiPreferences>(initialPreferences)
  const [systemPrefersDark, setSystemPrefersDark] = useState(initialSystemPrefersDark)

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = (event: MediaQueryListEvent): void => setSystemPrefersDark(event.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [])

  useEffect(() => {
    setActiveLocale(preferences.locale)
    document.documentElement.lang = preferences.locale
    document.documentElement.dataset.arkFontSize = preferences.fontSize
    document.documentElement.dataset.arkTheme = resolveTheme(preferences.theme, systemPrefersDark)
    window.localStorage.setItem(UI_PREFERENCES_STORAGE_KEY, JSON.stringify(preferences))
  }, [preferences, systemPrefersDark])

  const value = useMemo<I18nContextValue>(() => ({
    ...preferences,
    setLocale: (locale) => setPreferences((current) => ({ ...current, locale })),
    setFontSize: (fontSize) => setPreferences((current) => ({ ...current, fontSize })),
    setTheme: (theme) => setPreferences((current) => ({ ...current, theme })),
    t: (key, params) => translate(preferences.locale, key, params)
  }), [preferences])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext)
  // Developer diagnostic, not user copy: it can only fire on a wiring bug.
  if (!context) throw new Error('useI18n must be used inside I18nProvider')
  return context
}
