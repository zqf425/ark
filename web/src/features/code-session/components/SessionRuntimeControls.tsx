import { useEffect, useRef, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { compactThread, getThread, updateThread } from '../api/threads-api'
import { useProviders } from '../../settings/hooks/use-providers'
import { getRuntimeConfig } from '../../settings/api/settings-api'
import { canSaveRuntime, isRuntimeFormStale, type RuntimeOwner } from './session-runtime-guard'

const DEFAULT_RUNTIME: ThreadRuntimeSettings = {}
/** Shown until the global default resolves; never persisted on its own. */
const FALLBACK_AUTONOMY = 'workspace_write'

export function SessionRuntimeControls({ threadId }: { threadId: string }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const providers = useProviders()
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>(DEFAULT_RUNTIME)
  // Model suggestions come from the selected provider's inventory: the default
  // model plus everything batch-imported for it.
  const selectedProvider = providers.providers.find((provider) => provider.id === (runtime.provider_id ?? '')) ?? null
  const modelOptions = selectedProvider
    ? [selectedProvider.model, ...selectedProvider.models].filter((id) => id && id !== (runtime.model ?? ''))
    : []
  const [defaultAutonomy, setDefaultAutonomy] = useState(FALLBACK_AUTONOMY)
  const [saving, setSaving] = useState(false)
  const [compacting, setCompacting] = useState(false)
  /** Thread the form values were actually loaded for; `null` until a load lands. */
  const [loadedFor, setLoadedFor] = useState<RuntimeOwner>(null)
  const paintedThreadId = useRef(threadId)

  // Switching threads must blank the form in the same commit: reusing A's
  // workspace/provider/model while B loads would show (and offer to save) the
  // wrong thread's overrides.
  if (isRuntimeFormStale(paintedThreadId.current, threadId)) {
    paintedThreadId.current = threadId
    setRuntime(DEFAULT_RUNTIME)
    setLoadedFor(null)
    setSaving(false)
    setCompacting(false)
  }

  useEffect(() => {
    let active = true
    setLoadedFor(null)
    void getThread(threadId)
      .then((detail) => {
        if (!active) return
        setRuntime({ ...DEFAULT_RUNTIME, ...detail.runtime })
        setLoadedFor(threadId)
      })
      .catch((error: unknown) => {
        if (!active) return
        // A failed load leaves `loadedFor` null, so the form cannot be saved
        // over another thread's stored overrides.
        setLoadedFor(null)
        toast.push('error', t('session.runtime.title'), error instanceof Error ? error.message : String(error))
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
    // `toast` and `t` are stable enough for this load; including them would
    // re-run the fetch on every language switch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [threadId])

  const save = async (): Promise<void> => {
    // Drop the write instead of applying the displayed values to another thread.
    if (!canSaveRuntime(loadedFor, threadId)) return
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
        <label>
          {t('session.runtime.model')}
          <input list="ark-session-model-options" value={runtime.model ?? ''} onChange={(event) => setRuntime({ ...runtime, model: event.target.value || undefined })} />
          <datalist id="ark-session-model-options">
            {modelOptions.map((id) => <option key={id} value={id} />)}
          </datalist>
        </label>
        <label>{t('session.runtime.autonomy')}<select value={runtime.autonomy ?? defaultAutonomy} onChange={(event) => setRuntime({ ...runtime, autonomy: event.target.value })}><option value="strict">{t('session.runtime.strict')}</option><option value="workspace_write">{t('session.runtime.workspaceWrite')}</option><option value="unattended">{t('session.runtime.unattended')}</option></select></label>
        <Button size="sm" variant="primary" disabled={saving || !canSaveRuntime(loadedFor, threadId)} onClick={() => void save()}>{t('session.runtime.save')}</Button>
        <Button size="sm" disabled={compacting} onClick={() => void compact()}>{t('session.context.compact')}</Button>
      </div>
    </details>
  )
}
