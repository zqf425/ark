/**
 * Detailed runtime panel docked above the composer: workspace override and
 * context compaction. The quick overrides (model, permission mode, reasoning
 * effort) live in the composer toolbar (`ComposerRuntimeControls`); keeping
 * only the workspace field here means the panel's explicit save can never
 * clobber a value the toolbar just wrote (the PATCH merge keeps fields the
 * patch omits).
 */
import { useEffect, useRef, useState } from 'react'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { compactThread, getThread, updateThread } from '../api/threads-api'
import { pickWorkspaceFolder } from '../api/workspace-api'
import { canSaveRuntime, isRuntimeFormStale, type RuntimeOwner } from './session-runtime-guard'

const DEFAULT_RUNTIME: ThreadRuntimeSettings = {}

export function SessionRuntimeControls({ threadId }: { threadId: string }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const [runtime, setRuntime] = useState<ThreadRuntimeSettings>(DEFAULT_RUNTIME)
  const [saving, setSaving] = useState(false)
  const [compacting, setCompacting] = useState(false)
  const [picking, setPicking] = useState(false)
  /** Thread the form values were actually loaded for; `null` until a load lands. */
  const [loadedFor, setLoadedFor] = useState<RuntimeOwner>(null)
  const paintedThreadId = useRef(threadId)

  // Switching threads must blank the form in the same commit: reusing A's
  // workspace while B loads would show (and offer to save) the wrong thread's
  // overrides.
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
      // Only the workspace override travels: omitted fields keep their stored
      // values, so a toolbar change made after this panel loaded survives.
      await updateThread(threadId, { runtime: { workspace: runtime.workspace } })
      toast.push('success', t('session.runtime.title'))
    } catch (error) {
      toast.push('error', t('session.runtime.title'), error instanceof Error ? error.message : String(error))
    } finally {
      setSaving(false)
    }
  }

  const pick = async (): Promise<void> => {
    setPicking(true)
    try {
      // The dialog is modal on the host machine; the request resolves when the
      // user closes it. A cancelled dialog keeps the current input untouched.
      const result = await pickWorkspaceFolder()
      if (result.path) setRuntime((current) => ({ ...current, workspace: result.path }))
    } catch (error) {
      toast.push('error', t('session.runtime.workspace'), error instanceof Error ? error.message : String(error))
    } finally {
      setPicking(false)
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
        <Button size="sm" disabled={picking} onClick={() => void pick()}>{t('session.runtime.browse')}</Button>
        <Button size="sm" variant="primary" disabled={saving || !canSaveRuntime(loadedFor, threadId)} onClick={() => void save()}>{t('session.runtime.save')}</Button>
        <Button size="sm" disabled={compacting} onClick={() => void compact()}>{t('session.context.compact')}</Button>
      </div>
    </details>
  )
}
