/**
 * Settings page composition.
 *
 * Wiring only: each section owns its own hook and its own state. The page keeps
 * the shared `useSettings` / `useProviders` instances so the default-model picker
 * and the provider list cannot disagree.
 */
import { ErrorBoundary } from '../../../shared/ui'
import { useSettings } from '../hooks/use-settings'
import { useProviders } from '../hooks/use-providers'
import { GeneralSection } from './GeneralSection'
import { TuningSection } from './TuningSection'
import { SubagentSection } from './SubagentSection'
import { ProvidersSection } from './ProvidersSection'
import { McpSection } from './McpSection'
import { SkillsSection } from './SkillsSection'
import '../settings.css'

export function SettingsPage(): JSX.Element {
  const settings = useSettings()
  const providers = useProviders()

  return (
    <div className="ark-settings">
      <div className="ark-settings__scroll">
        <div className="ark-settings__body">
          <ErrorBoundary label="通用设置崩溃">
            <GeneralSection settings={settings} providers={providers} />
          </ErrorBoundary>

          <ErrorBoundary label="运行调优设置崩溃">
            <TuningSection settings={settings} />
          </ErrorBoundary>

          <ErrorBoundary label="子 agent 设置崩溃">
            <SubagentSection settings={settings} />
          </ErrorBoundary>

          <ErrorBoundary label="供应商设置崩溃">
            <ProvidersSection providers={providers} />
          </ErrorBoundary>

          <ErrorBoundary label="MCP 设置崩溃">
            <McpSection />
          </ErrorBoundary>

          <ErrorBoundary label="Skills 设置崩溃">
            <SkillsSection />
          </ErrorBoundary>
        </div>
      </div>
    </div>
  )
}
