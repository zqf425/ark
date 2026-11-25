/** Local settings-page navigation only. Runtime configuration remains in each section hook. */
import type { MessageKey } from '../../../shared/i18n/messages'

export type SettingsSectionId =
  | 'general'
  | 'runtime'
  | 'providers'
  | 'integrations'
  | 'plugins'
  | 'memory'
  | 'schedules'
  | 'im'
  | 'skills'

export type SettingsSectionGroupId = 'foundation' | 'capabilities' | 'automation'

export type SettingsSection = {
  id: SettingsSectionId
  icon: string
  labelKey: MessageKey
  detailKey: MessageKey
  group: SettingsSectionGroupId
}

export const SETTINGS_SECTION_GROUPS: readonly { id: SettingsSectionGroupId; labelKey: MessageKey }[] = [
  { id: 'foundation', labelKey: 'settings.group.foundation' },
  { id: 'capabilities', labelKey: 'settings.group.capabilities' },
  { id: 'automation', labelKey: 'settings.group.automation' }
]

export const SETTINGS_SECTIONS: readonly SettingsSection[] = [
  { id: 'general', group: 'foundation', icon: 'settings', labelKey: 'settings.nav.general.label', detailKey: 'settings.nav.general.detail' },
  { id: 'runtime', group: 'foundation', icon: 'terminal', labelKey: 'settings.nav.runtime.label', detailKey: 'settings.nav.runtime.detail' },
  { id: 'providers', group: 'foundation', icon: 'layers', labelKey: 'settings.nav.providers.label', detailKey: 'settings.nav.providers.detail' },
  { id: 'integrations', group: 'capabilities', icon: 'code', labelKey: 'settings.nav.integrations.label', detailKey: 'settings.nav.integrations.detail' },
  { id: 'plugins', group: 'capabilities', icon: 'plugin', labelKey: 'settings.nav.plugins.label', detailKey: 'settings.nav.plugins.detail' },
  { id: 'memory', group: 'capabilities', icon: 'memory', labelKey: 'settings.nav.memory.label', detailKey: 'settings.nav.memory.detail' },
  { id: 'schedules', group: 'automation', icon: 'clock', labelKey: 'settings.nav.schedules.label', detailKey: 'settings.nav.schedules.detail' },
  { id: 'im', group: 'automation', icon: 'chat', labelKey: 'settings.nav.im.label', detailKey: 'settings.nav.im.detail' },
  { id: 'skills', group: 'capabilities', icon: 'spark', labelKey: 'settings.nav.skills.label', detailKey: 'settings.nav.skills.detail' }
]

export function isSettingsSection(value: string): value is SettingsSectionId {
  return SETTINGS_SECTIONS.some((section) => section.id === value)
}
