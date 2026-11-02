/**
 * Plugins, capabilities and the built-in tool catalog, all read-only
 * projections the runtime already serves (smart-ark's "assistant status"
 * diagnostics, scoped to what arkd implements).
 *
 * State is local: three independent GETs, each with its own load/error channel
 * so one failing surface cannot blank the others.
 */
import { useEffect, useState } from 'react'
import type { CapabilitiesResponse, PluginHostReport } from '../../../shared/contracts'
import type { RuntimeToolsReport } from '../../../shared/contracts/web-only'
import { useI18n } from '../../../shared/i18n'
import { getCapabilities, getPluginsReport, getToolsReport } from '../api/runtime-plugins-api'
import { Notice, SettingsSection, Skeleton } from './SectionShell'

type Loadable<T> = { data: T | null; error: string | null }

const EMPTY: Loadable<never> = { data: null, error: null }

function useDiagnostics<T>(load: (signal: AbortSignal) => Promise<T>): Loadable<T> {
  const [state, setState] = useState<Loadable<T>>(EMPTY as Loadable<T>)
  useEffect(() => {
    const controller = new AbortController()
    load(controller.signal)
      .then((data) => setState({ data, error: null }))
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setState({ data: null, error: error instanceof Error ? error.message : String(error) })
      })
    return () => controller.abort()
    // The loader identity is stable per call site; a dep would re-fetch on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  return state
}

function CardBody({ state, rows }: { state: Loadable<unknown>; rows: number }): JSX.Element | null {
  const { t } = useI18n()
  if (state.error) {
    return (
      <Notice tone="error" title={t('settings.plugins.loadFailed')}>
        {state.error}
      </Notice>
    )
  }
  if (!state.data) return <Skeleton rows={rows} />
  return null
}

export function PluginsSection(): JSX.Element {
  const { t } = useI18n()
  const capabilities = useDiagnostics<CapabilitiesResponse>(getCapabilities)
  const plugins = useDiagnostics<PluginHostReport>(getPluginsReport)
  const tools = useDiagnostics<RuntimeToolsReport>(getToolsReport)

  return (
    <>
      <SettingsSection title={t('settings.plugins.capabilities.title')} desc={t('settings.plugins.capabilities.desc')}>
        <CardBody state={capabilities} rows={2} />
        {capabilities.data ? (
          <div className="ark-settings__tools">
            {capabilities.data.capabilities.map((capability) => (
              <span key={capability} className="ark-chip ark-chip--live">
                {capability}
              </span>
            ))}
          </div>
        ) : null}
        {capabilities.data ? (
          <p className="ark-settings__hint">
            {t('settings.plugins.capabilityCatalog', {
              pages: capabilities.data.catalog.pages.length,
              panels: capabilities.data.catalog.panels.length
            })}
          </p>
        ) : null}
      </SettingsSection>

      <SettingsSection title={t('settings.plugins.title')} desc={t('settings.plugins.desc')}>
        <CardBody state={plugins} rows={3} />
        {plugins.data ? (
          <>
            {plugins.data.failed.length > 0 || plugins.data.skipped.length > 0 ? (
              <Notice tone="error" title={t('settings.plugins.brokenTitle')}>
                {t('settings.plugins.brokenBody', {
                  failed: plugins.data.failed.join(t('common.listSeparator')) || '—',
                  skipped: plugins.data.skipped.join(t('common.listSeparator')) || '—'
                })}
              </Notice>
            ) : null}
            {plugins.data.plugins.length === 0 ? (
              <p className="ark-settings__hint">{t('settings.plugins.empty')}</p>
            ) : (
              <div className="ark-settings__list">
                {plugins.data.plugins.map((plugin) => (
                  <details key={plugin.id} className="ark-card">
                    <summary className="ark-card__head">
                      <span className="ark-card__name">{plugin.id}</span>
                      <span className="ark-chip ark-chip--muted">{plugin.kind}</span>
                      <span className="ark-card__meta">
                        v{plugin.version}
                      </span>
                    </summary>
                    <div className="ark-settings__stack">
                      <p className="ark-settings__hint">
                        {t('settings.plugins.contributions', {
                          pages: plugin.pages.length,
                          panels: plugin.panels.length,
                          tools: plugin.tools.length
                        })}
                      </p>
                      {plugin.pages.length > 0 ? (
                        <p className="ark-settings__code">{plugin.pages.join(t('common.listSeparator'))}</p>
                      ) : null}
                      {plugin.panels.length > 0 ? (
                        <p className="ark-settings__code">{plugin.panels.join(t('common.listSeparator'))}</p>
                      ) : null}
                      {plugin.tools.length > 0 ? (
                        <p className="ark-settings__code">{plugin.tools.join(t('common.listSeparator'))}</p>
                      ) : null}
                    </div>
                  </details>
                ))}
              </div>
            )}
          </>
        ) : null}
      </SettingsSection>

      <SettingsSection title={t('settings.plugins.tools.title')} desc={t('settings.plugins.tools.desc')}>
        <CardBody state={tools} rows={3} />
        {tools.data ? (
          <>
            <p className="ark-settings__hint">
              {t('settings.plugins.toolsCount', { count: tools.data.tools.length, version: tools.data.version })}
            </p>
            <div className="ark-settings__list">
              {tools.data.tools.map((tool) => (
                <details key={tool.name} className="ark-card">
                  <summary className="ark-card__head">
                    <span className="ark-card__name ark-settings__code">{tool.name}</span>
                  </summary>
                  <pre className="ark-tool__output">{JSON.stringify(tool.schema, null, 2)}</pre>
                </details>
              ))}
            </div>
          </>
        ) : null}
      </SettingsSection>
    </>
  )
}
