import { useEffect, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { getThread, updateThread } from '../api/threads-api'
import { useProviders } from '../../settings/hooks/use-providers'

const DEFAULT_RUNTIME: ThreadRuntimeSettings = { autonomy: 'workspace_write' }

export function SessionRuntimeControls({ threadId }: { threadId: string }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const providers = useProviders()
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>(DEFAULT_RUNTIME)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    let active = true
    void getThread(threadId).then((detail) => {
      if (active) setRuntime({ ...DEFAULT_RUNTIME, ...detail.runtime })
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

  return (
    <details className="ark-session-runtime">
      <summary>{t('session.runtime.title')}</summary>
      <div className="ark-session-runtime__fields">
        <label>{t('session.runtime.workspace')}<input value={runtime.workspace ?? ''} onChange={(event) => setRuntime({ ...runtime, workspace: event.target.value || undefined })} /></label>
        <label>{t('session.runtime.provider')}<select value={runtime.provider_id ?? ''} onChange={(event) => setRuntime({ ...runtime, provider_id: event.target.value || undefined })}><option value="">—</option>{providers.providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.label ?? provider.id}</option>)}</select></label>
        <label>{t('session.runtime.model')}<input value={runtime.model ?? ''} onChange={(event) => setRuntime({ ...runtime, model: event.target.value || undefined })} /></label>
        <label>{t('session.runtime.autonomy')}<select value={runtime.autonomy ?? 'workspace_write'} onChange={(event) => setRuntime({ ...runtime, autonomy: event.target.value })}><option value="strict">{t('session.runtime.strict')}</option><option value="workspace_write">{t('session.runtime.workspaceWrite')}</option><option value="unattended">{t('session.runtime.unattended')}</option></select></label>
        <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()}>{t('session.runtime.save')}</Button>
      </div>
    </details>
  )
}
