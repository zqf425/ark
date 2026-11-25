/**
 * New-task page: describe the goal, start.
 *
 * The input area IS the session composer (same component as the canvas dock),
 * mounted with `threadId = null`: workspace / model / permission / reasoning
 * pills, attachments and skill mentions all work exactly like an open session.
 * Picking a model or workspace only stages the override; the send action
 * creates the thread (`POST /v1/threads` with the staged workspace), commits
 * the remaining overrides (`PATCH /v1/threads/{id}`), associates the picked
 * attachments and starts the first turn — one delivery, like every composer
 * send. A delivered send jumps straight into the workbench with the new
 * thread selected: the store's active thread is set before the delivery, and
 * the view switches to `code` only after the whole chain lands, so a failure
 * keeps the user on this page with draft and picks intact.
 */
import { useRef, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { useI18n } from '../../../shared/i18n'
import { Icon, useToast } from '../../../shared/ui'
import type { ThreadRuntimeSettings } from '../../../shared/contracts'
import { createThread, updateThread } from '../../code-session/api/threads-api'
import { startTurn } from '../../code-session/api/turns-api'
import { associateAttachment } from '../../code-session/api/attachments-api'
import { useCodeSessionStore } from '../../code-session/state/store'
import { NEW_TASK_DRAFT_KEY, Composer, type ComposerSubmit } from '../../code-session/components/Composer'
import { useViewStore } from '../../workbench/state/view-slice'
import type { ConsoleMeta } from '../hooks/use-console-meta'

/** Props stay `meta`-compatible with the other console scenes (unused here). */
export function NewTaskPage(_props: { meta?: ConsoleMeta }): JSX.Element {
  const { t } = useI18n()
  const push = useToast().push
  const setView = useViewStore((s) => s.setView)
  const setActiveThread = useCodeSessionStore((s) => s.setActiveThread)
  const upsertThread = useCodeSessionStore((s) => s.upsertThread)

  const [busy, setBusy] = useState(false)
  // React state updates are batched; Enter twice before a rerender must still
  // create only one thread.
  const inFlightRef = useRef(false)
  // Overrides picked on the page before the thread exists. Committed at start
  // time: the workspace seeds the create call, the rest merge into the fresh
  // thread's runtime. Cleared on success so the next task starts neutral.
  const [staged, setStaged] = useState<ThreadRuntimeSettings>({})

  const stage = (patch: ThreadRuntimeSettings): void => {
    setStaged((current) => ({ ...current, ...patch }))
  }

  const start: ComposerSubmit = async (text, attachmentIds) => {
    if (busy || inFlightRef.current) return false
    inFlightRef.current = true
    setBusy(true)
    try {
      // A staged workspace is validated server-side (the same
      // `WorkspaceService::validate` rule as the composer's pick), so an
      // invalid path fails the create instead of seeding a dead override.
      const created = await createThread({ title: t('thread.list.defaultTitle'), ...(staged.workspace ? { workspace: staged.workspace } : {}) })
      upsertThread(created)
      setActiveThread(created.thread_id)
      // merge_runtime keeps omitted fields, so only what the user picked lands.
      const overrides: ThreadRuntimeSettings = {}
      if (staged.provider_id !== undefined) overrides.provider_id = staged.provider_id
      if (staged.model !== undefined) overrides.model = staged.model
      if (staged.autonomy !== undefined) overrides.autonomy = staged.autonomy
      if (staged.reasoning_effort !== undefined) overrides.reasoning_effort = staged.reasoning_effort
      if (Object.keys(overrides).length > 0) {
        await updateThread(created.thread_id, { runtime: overrides })
      }
      // The runtime rejects turn starts that carry unassociated attachment
      // ids, so every picked id is linked before the turn starts.
      for (const id of attachmentIds) {
        await associateAttachment(created.thread_id, id)
      }
      if (text) {
        await startTurn(created.thread_id, { text, attachment_ids: attachmentIds })
      }
      // Delivered: the page's staged picks are now the thread's stored state.
      setStaged({})
      useCodeSessionStore.getState().clearDraft(NEW_TASK_DRAFT_KEY)
      // Jump into the workbench: the thread is already active in the store, so
      // the canvas opens this session and the rail shows it under its project.
      setView('code')
      return true
    } catch (error) {
      push('error', t('console.new.failed'), describeError(error, translateNow))
      return false
    } finally {
      inFlightRef.current = false
      setBusy(false)
    }
  }

  return (
    <section className="console-new" aria-label={t('console.new.title')}>
      <div className="console-new__wrap">
        <div className="console-new__kicker">
          <span className="console-mark console-mark--sm" aria-hidden="true">A</span>
          {t('console.new.kicker')}
        </div>
        <h2>{t('console.new.title')}</h2>
        <p className="console-new__sub">{t('console.new.subtitle')}</p>

        {/*
          The session composer, unkeyed: the new-task page never remounts it on
          a thread switch (the page owns the draft under NEW_TASK_DRAFT_KEY and
          the staged overrides live in `staged`), so pills and attachments keep
          their state while the user types. A delivered send sets the active
          thread and switches the shell's view to `code`, opening the session
          in the workbench's thread list + canvas.
        */}
        <Composer
          threadId={null}
          onPendingStart={start}
          stagedRuntime={staged}
          onStagedChange={stage}
        />

        <div className="console-new__recent">
          <Icon name="clock" size={12} />
          {t('console.new.recentHint')}
        </div>
      </div>
    </section>
  )
}
