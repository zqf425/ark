import { describe, expect, it } from 'vitest'
import { SETTINGS_SECTIONS, isSettingsSection } from './settings-navigation'

describe('settings navigation', () => {
  it('exposes each settings capability once and rejects unknown local navigation state', () => {
    expect(SETTINGS_SECTIONS.map((section) => section.id)).toEqual(['general', 'runtime', 'providers', 'integrations', 'skills'])
    expect(isSettingsSection('providers')).toBe(true)
    expect(isSettingsSection('unknown')).toBe(false)
  })
})
