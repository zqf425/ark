/**
 * Locale access for code that runs outside React.
 *
 * Pure helpers (reducers, formatters, API result mapping) cannot call
 * `useI18n`, but they still must not print a hardcoded language. `I18nProvider`
 * publishes the active locale here on every change, and `translateNow` reads it.
 * Components keep using `t` from context so they re-render on a language switch;
 * this module is only the fallback for non-React call sites.
 */
import { translate, type MessageKey, type MessageParams } from './messages'
import { DEFAULT_UI_PREFERENCES, type UiLocale } from './preferences'

let activeLocale: UiLocale = DEFAULT_UI_PREFERENCES.locale

/** Called by `I18nProvider`; not part of the component API. */
export function setActiveLocale(locale: UiLocale): void {
  activeLocale = locale
}

export function getActiveLocale(): UiLocale {
  return activeLocale
}

/** Translate with the currently active locale (non-React call sites only). */
export function translateNow(key: MessageKey, params?: MessageParams): string {
  return translate(activeLocale, key, params)
}
