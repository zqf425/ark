/** Local settings-page navigation only. Runtime configuration remains in each section hook. */
import type { MessageKey } from '../../../shared/i18n/messages'

export type SettingsSectionId = 'general' | 'runtime' | 'providers' | 'integrations' | 'skills'

export type SettingsSection = {
  id: SettingsSectionId
  labelKey: MessageKey
  detailKey: MessageKey
}

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  { id: 'general', labelKey: 'settings.nav.general.label', detailKey: 'settings.nav.general.detail' },
  { id: 'runtime', labelKey: 'settings.nav.runtime.label', detailKey: 'settings.nav.runtime.detail' },
  { id: 'providers', labelKey: 'settings.nav.providers.label', detailKey: 'settings.nav.providers.detail' },
  { id: 'integrations', labelKey: 'settings.nav.integrations.label', detailKey: 'settings.nav.integrations.detail' },
  { id: 'skills', labelKey: 'settings.nav.skills.label', detailKey: 'settings.nav.skills.detail' }
]

export function isSettingsSection(value: string): value is SettingsSectionId {
  return SETTINGS_SECTIONS.some((section) => section.id === value)
}
