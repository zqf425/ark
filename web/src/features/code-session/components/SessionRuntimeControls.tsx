import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { compactThread, getThread, updateThread } from '../api/threads-api'
import { useProviders } from '../../settings/hooks/use-providers'
import { getRuntimeConfig } from '../../settings/api/settings-api'

const DEFAULT_RUNTIME: ThreadRuntimeSettings = {}
/** Shown until the global default resolves; never persisted on its own. */
const FALLBACK_AUTONOMY = 'workspace_write'

export function SessionRuntimeControls({ threadId }: { threadId: string }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const providers = useProviders()
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>(DEFAULT_RUNTIME)
  const [defaultAutonomy, setDefaultAutonomy] = useState(FALLBACK_AUTONOMY)
  const [saving, setSaving] = useState(false)
  const [compacting, setCompacting] = useState(false)

  useEffect(() => {
    let active = true
    void getThread(threadId).then((detail) => {
      if (active) setRuntime({ ...DEFAULT_RUNTIME, ...detail.runtime })
    })
    // The runtime falls back to the global default when a thread has no
    // override, so the control must show that same value: showing a hardcoded
    // mode would misreport the effective permission.
    void getRuntimeConfig()
      .then((config) => {
        if (active) setDefaultAutonomy(config.default_autonomy)
      })
      .catch(() => {
        // Keep the fallback: an unreadable config must not block the panel.
      })
    return () => { active = false }
  }, [threadId])

  const save = async (): Promise<void> => {
    setSaving(true)
    try {
      await updateThread(threadId, { runtime })
      toast.push('success', t('session.runtime.title'))
    } catch (error) {
      toast.push('error', t('session.runtime.title'), error instanceof Error ? error.message : String(error))
    } finally {
      setSaving(false)
    }
  }

  const compact = async (): Promise<void> => {
    setCompacting(true)
    try {
      await compactThread(threadId)
      toast.push('success', t('session.context.compacted'))
    } catch (error) {
      toast.push('error', t('session.context.compact'), error instanceof Error ? error.message : String(error))
    } finally {
      setCompacting(false)
    }
  }

  return (
    <details className="ark-session-runtime">
      <summary>{t('session.runtime.title')}</summary>
      <div className="ark-session-runtime__fields">
        <label>{t('session.runtime.workspace')}<input value={runtime.workspace ?? ''} onChange={(event) => setRuntime({ ...runtime, workspace: event.target.value || undefined })} /></label>
        <label>{t('session.runtime.provider')}<select value={runtime.provider_id ?? ''} onChange={(event) => setRuntime({ ...runtime, provider_id: event.target.value || undefined })}><option value="">—</option>{providers.providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.label ?? provider.id}</option>)}</select></label>
        <label>{t('session.runtime.model')}<input value={runtime.model ?? ''} onChange={(event) => setRuntime({ ...runtime, model: event.target.value || undefined })} /></label>
        <label>{t('session.runtime.autonomy')}<select value={runtime.autonomy ?? defaultAutonomy} onChange={(event) => setRuntime({ ...runtime, autonomy: event.target.value })}><option value="strict">{t('session.runtime.strict')}</option><option value="workspace_write">{t('session.runtime.workspaceWrite')}</option><option value="unattended">{t('session.runtime.unattended')}</option></select></label>
        <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()}>{t('session.runtime.save')}</Button>
        <Button size="sm" disabled={compacting} onClick={() => void compact()}>{t('session.context.compact')}</Button>
      </div>
    </details>
  )
}
