/**
 * The message catalog: one flat `key -> [zh-CN, en-US]` table.
 *
 * Per-feature catalogs live in `./catalogs/*` so no single file has to grow
 * without bound and a feature owner can find its own copy. `MESSAGES` is the
 * only thing the rest of the app imports; `MessageKey` is derived from it, so
 * an unknown key is a compile error rather than a silent English fallback.
 */
import type { UiLocale } from './preferences'
import { COMMON_MESSAGES } from './catalogs/common'
import { ERROR_MESSAGES } from './catalogs/errors'
import { SESSION_MESSAGES } from './catalogs/session'
import { SESSION_CANVAS_MESSAGES } from './catalogs/session-canvas'
import { SETTINGS_CORE_MESSAGES } from './catalogs/settings-core'
import { SETTINGS_GENERAL_MESSAGES } from './catalogs/settings-general'
import { SETTINGS_INTEGRATIONS_MESSAGES } from './catalogs/settings-integrations'
import { SETTINGS_PROVIDERS_MESSAGES } from './catalogs/settings-providers'
import { SETTINGS_RUNTIME_MESSAGES } from './catalogs/settings-runtime'
import { THREAD_LIST_MESSAGES } from './catalogs/thread-list'
import { WORKBENCH_MESSAGES } from './catalogs/workbench'

export const MESSAGES = {
  ...COMMON_MESSAGES,
  ...ERROR_MESSAGES,
  ...SESSION_MESSAGES,
  ...SESSION_CANVAS_MESSAGES,
  ...SETTINGS_CORE_MESSAGES,
  ...SETTINGS_GENERAL_MESSAGES,
  ...SETTINGS_INTEGRATIONS_MESSAGES,
  ...SETTINGS_PROVIDERS_MESSAGES,
  ...SETTINGS_RUNTIME_MESSAGES,
  ...THREAD_LIST_MESSAGES,
  ...WORKBENCH_MESSAGES
} as const

export type MessageKey = keyof typeof MESSAGES

/** Values substituted into `{placeholder}` slots of a message. */
export type MessageParams = Record<string, string | number>

/**
 * `translate` is the single lookup: locale picks the column, and `params`
 * fills `{name}` slots. A missing param leaves its slot untouched so a bad
 * call is visible instead of silently printing `undefined`.
 */
export function translate(locale: UiLocale, key: MessageKey, params?: MessageParams): string {
  const template = MESSAGES[key][locale === 'zh-CN' ? 0 : 1]
  if (!params) return template
  return template.replace(/\{(\w+)\}/g, (slot, name: string) => {
    const value = params[name]
    return value === undefined ? slot : String(value)
  })
}
