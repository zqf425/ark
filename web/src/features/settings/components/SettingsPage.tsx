/**
 * Settings page composition.
 *
 * Wiring only: each section owns its own hook and its own state. The page keeps
 * the shared `useSettings` / `useProviders` instances so the default-model picker
 * and the provider list cannot disagree.
 */
import { useState } from 'react'
import { ErrorBoundary } from '../../../shared/ui'
import { useSettings } from '../hooks/use-settings'
import { useProviders } from '../hooks/use-providers'
import { GeneralSection } from './GeneralSection'
import { TuningSection } from './TuningSection'
import { SubagentSection } from './SubagentSection'
import { ProvidersSection } from './ProvidersSection'
import { McpSection } from './McpSection'
import { SkillsSection } from './SkillsSection'
import { SETTINGS_SECTIONS, type SettingsSectionId } from './settings-navigation'
import '../settings.css'
import '../settings-agent.css'

export function SettingsPage(): JSX.Element {
  const settings = useSettings()
  const providers = useProviders()
  const [activeSection, setActiveSection] = useState<SettingsSectionId>('general')
  const current = SETTINGS_SECTIONS.find((section) => section.id === activeSection) ?? SETTINGS_SECTIONS[0]

  const body = (() => {
    switch (activeSection) {
      case 'general':
        return (
          <ErrorBoundary label="通用设置崩溃">
            <GeneralSection settings={settings} providers={providers} />
          </ErrorBoundary>
        )
      case 'runtime':
        return (
          <>
            <ErrorBoundary label="运行调优设置崩溃">
              <TuningSection settings={settings} />
            </ErrorBoundary>
            <ErrorBoundary label="子 agent 设置崩溃">
              <SubagentSection settings={settings} />
            </ErrorBoundary>
          </>
        )
      case 'providers':
        return (
          <ErrorBoundary label="供应商设置崩溃">
            <ProvidersSection providers={providers} />
          </ErrorBoundary>
        )
      case 'integrations':
        return (
          <ErrorBoundary label="MCP 设置崩溃">
            <McpSection />
          </ErrorBoundary>
        )
      case 'skills':
        return (
          <ErrorBoundary label="Skills 设置崩溃">
            <SkillsSection />
          </ErrorBoundary>
        )
    }
  })()

  return (
    <div className="ark-settings" aria-label="设置">
      <aside className="ark-settings__rail" aria-label="设置分类">
        <div className="ark-settings__rail-head">
          <strong>设置</strong>
          <p>配置本地运行时与外部能力。</p>
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
              <span>{section.label}</span>
              <small>{section.detail}</small>
            </button>
          ))}
        </nav>
      </aside>

      <main className="ark-settings__scroll">
        <div className="ark-settings__body">
          <header className="ark-settings__hero">
            <h1>{current?.label}</h1>
            <p>{current?.detail}</p>
          </header>
          {body}
        </div>
      </main>
    </div>
  )
}
