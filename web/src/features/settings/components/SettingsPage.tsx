/**
 * Settings page composition.
 *
 * Wiring only: each section owns its own hook and its own state. The page keeps
 * the shared `useSettings` / `useProviders` instances so the default-model picker
 * and the provider list cannot disagree.
 */
import { useEffect, useState } from 'react'
import { ErrorBoundary } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useViewStore } from '../../workbench/state/view-slice'
import { Icon } from '../../../shared/ui'
import { useSettings } from '../hooks/use-settings'
import { useProviders } from '../hooks/use-providers'
import { GeneralSection } from './GeneralSection'
import { TuningSection } from './TuningSection'
import { SubagentSection } from './SubagentSection'
import { ProvidersSection } from './ProvidersSection'
import { McpSection } from './McpSection'
import { PluginsSection } from './PluginsSection'
import { MemorySection } from './MemorySection'
import { SkillsSection } from './SkillsSection'
import { ScheduleSection } from './ScheduleSection'
import { ImSection } from './ImSection'
import { SETTINGS_SECTIONS, type SettingsSectionId } from './settings-navigation'
import '../settings.css'
import '../settings-import-dialog.css'
import '../settings-agent.css'

export function SettingsPage(): JSX.Element {
  const { t } = useI18n()
  const settings = useSettings()
  const providers = useProviders()
  const setView = useViewStore((state) => state.setView)
  const [activeSection, setActiveSection] = useState<SettingsSectionId>('general')
  const current = SETTINGS_SECTIONS.find((section) => section.id === activeSection) ?? SETTINGS_SECTIONS[0]

  // A caller that jumps to a specific section (schedule creation → task list)
  // wins once; without a pending request the page opens on its default
  // section, exactly as it did before the request channel existed.
  const consumeSettingsSection = useViewStore((state) => state.consumeSettingsSection)
  useEffect(() => {
    const requested = consumeSettingsSection() as SettingsSectionId | null
    if (requested && SETTINGS_SECTIONS.some((section) => section.id === requested)) {
      setActiveSection(requested)
    }
  }, [consumeSettingsSection])

  const body = (() => {
    switch (activeSection) {
      case 'general':
        return (
          <ErrorBoundary label={t('settings.boundary.general')}>
            <GeneralSection settings={settings} providers={providers} />
          </ErrorBoundary>
        )
      case 'runtime':
        return (
          <>
            <ErrorBoundary label={t('settings.boundary.runtime')}>
              <TuningSection settings={settings} />
            </ErrorBoundary>
            <ErrorBoundary label={t('settings.boundary.subagents')}>
              <SubagentSection settings={settings} />
            </ErrorBoundary>
          </>
        )
      case 'providers':
        return (
          <ErrorBoundary label={t('settings.boundary.providers')}>
            <ProvidersSection providers={providers} />
          </ErrorBoundary>
        )
      case 'integrations':
        return (
          <ErrorBoundary label={t('settings.boundary.mcp')}>
            <McpSection />
          </ErrorBoundary>
        )
      case 'plugins':
        return (
          <ErrorBoundary label={t('settings.boundary.plugins')}>
            <PluginsSection />
          </ErrorBoundary>
        )
      case 'memory':
        return (
          <ErrorBoundary label={t('settings.boundary.memory')}>
            <MemorySection />
          </ErrorBoundary>
        )
      case 'skills':
        return (
          <ErrorBoundary label={t('settings.boundary.skills')}>
            <SkillsSection settings={settings} />
          </ErrorBoundary>
        )
      case 'schedules':
        return (
          <ErrorBoundary label={t('settings.boundary.schedules')}>
            <ScheduleSection settings={settings} providers={providers} />
          </ErrorBoundary>
        )
      case 'im':
        return (
          <ErrorBoundary label={t('settings.im.boundary')}>
            <ImSection settings={settings} providers={providers} />
          </ErrorBoundary>
        )
    }
  })()

  return (
    <div className="ark-settings" aria-label={t('settings.title')}>
      <aside className="ark-settings__rail" aria-label={t('settings.title')}>
        <div className="ark-settings__rail-head">
          <strong>{t('settings.title')}</strong>
          <p>{t('settings.description')}</p>
        </div>
        <nav className="ark-settings__nav">
          {SETTINGS_SECTIONS.map((section) => (
            <button
              key={section.id}
              type="button"
              className={section.id === activeSection ? 'is-active' : ''}
              aria-current={section.id === activeSection ? 'page' : undefined}
              onClick={() => setActiveSection(section.id)}
            >
              <span className="ark-settings__nav-icon"><Icon name={section.icon} size={15} /></span>
              <span className="ark-settings__nav-copy">
                <span>{t(section.labelKey)}</span>
                <small>{t(section.detailKey)}</small>
              </span>
            </button>
          ))}
        </nav>
      </aside>

      <main className="ark-settings__scroll">
        <div className="ark-settings__body">
          <header className="ark-settings__hero ark-settings__hero--row">
            <div>
              <h1>{current ? t(current.labelKey) : ''}</h1>
              <p>{current ? t(current.detailKey) : ''}</p>
            </div>
            {/* The scene-level exit back to the code workbench: visible from
                every section, so leaving settings never depends on the top-bar
                gear or remembering ⌘K. */}
            <button
              type="button"
              className="ark-btn ark-btn--sm ark-settings__back"
              onClick={() => setView('code')}
              aria-label={t('settings.backToWorkbench')}
              title={t('settings.backToWorkbench')}
            >
              <Icon name="undo" size={13} />
              <span className="ark-settings__back-label">{t('settings.backToWorkbench')}</span>
            </button>
          </header>
          {body}
        </div>
      </main>
    </div>
  )
}
