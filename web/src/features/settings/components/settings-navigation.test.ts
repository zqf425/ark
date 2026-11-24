import { describe, expect, it } from 'vitest'
import { SETTINGS_SECTIONS, isSettingsSection } from './settings-navigation'

describe('settings navigation', () => {
  it('exposes each settings capability once and rejects unknown local navigation state', () => {
    expect(SETTINGS_SECTIONS.map((section) => section.id)).toEqual(['general', 'runtime', 'providers', 'integrations', 'plugins', 'memory', 'schedules', 'im', 'skills'])
    expect(SETTINGS_SECTIONS[0]).toMatchObject({ labelKey: 'settings.nav.general.label', detailKey: 'settings.nav.general.detail' })
    expect(isSettingsSection('providers')).toBe(true)
    expect(isSettingsSection('plugins')).toBe(true)
    expect(isSettingsSection('memory')).toBe(true)
    expect(isSettingsSection('schedules')).toBe(true)
    expect(isSettingsSection('im')).toBe(true)
    expect(isSettingsSection('unknown')).toBe(false)
  })

  it('gives every settings section a distinct scannable icon', () => {
    expect(SETTINGS_SECTIONS.map((section) => section.icon)).toEqual([
      'settings',
      'terminal',
      'layers',
      'code',
      'plugin',
      'memory',
      'clock',
      'chat',
      'spark'
    ])
    expect(new Set(SETTINGS_SECTIONS.map((section) => section.icon)).size).toBe(SETTINGS_SECTIONS.length)
  })
})
