/**
 * New-task page: pick an intent, describe the goal, start.
 *
 * The input area IS the session composer (same component as the canvas dock),
 * mounted with `threadId = null`: workspace / model / permission / reasoning
 * pills, attachments and skill mentions all work exactly like an open session.
 * Picking a model or workspace only stages the override; the send action
 * creates the thread (`POST /v1/threads` with the staged workspace), commits
 * the remaining overrides (`PATCH /v1/threads/{id}`), associates the picked
 * attachments and starts the first turn — one delivery, like every composer
 * send. Review still switches to the changes review page without a thread.
 */
import { useState } from 'react'
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

type Mode = 'code' | 'plan' | 'review'

const MODES: { id: Mode; icon: string; labelKey: 'console.new.modeCode' | 'console.new.modePlan' | 'console.new.modeReview'; hintKey: 'console.new.modeCodeHint' | 'console.new.modePlanHint' | 'console.new.modeReviewHint' }[] = [
  { id: 'code', icon: 'code', labelKey: 'console.new.modeCode', hintKey: 'console.new.modeCodeHint' },
  { id: 'plan', icon: 'list', labelKey: 'console.new.modePlan', hintKey: 'console.new.modePlanHint' },
  { id: 'review', icon: 'diff', labelKey: 'console.new.modeReview', hintKey: 'console.new.modeReviewHint' }
]

/** Props stay `meta`-compatible with the other console scenes (unused here). */
export function NewTaskPage(_props: { meta?: ConsoleMeta }): JSX.Element {
  const { t } = useI18n()
  const push = useToast().push
  const setView = useViewStore((s) => s.setView)
  const setActiveThread = useCodeSessionStore((s) => s.setActiveThread)
  const upsertThread = useCodeSessionStore((s) => s.upsertThread)

  const [mode, setMode] = useState<Mode>('code')
  const [busy, setBusy] = useState(false)
  // Overrides picked on the page before the thread exists. Committed at start
  // time: the workspace seeds the create call, the rest merge into the fresh
  // thread's runtime. Cleared on success so the next task starts neutral.
  const [staged, setStaged] = useState<ThreadRuntimeSettings>({})

  const stage = (patch: ThreadRuntimeSettings): void => {
    setStaged((current) => ({ ...current, ...patch }))
  }

  const start: ComposerSubmit = async (text, attachmentIds) => {
    if (busy) return false
    if (mode === 'review') {
      setView('review')
      return true
    }
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
      if (mode === 'code' && text) {
        await startTurn(created.thread_id, { text, attachment_ids: attachmentIds })
      }
      // Delivered: the page's staged picks are now the thread's stored state.
      setStaged({})
      useCodeSessionStore.getState().clearDraft(NEW_TASK_DRAFT_KEY)
      setView('code')
      return true
    } catch (error) {
      push('error', t('console.new.failed'), describeError(error, translateNow))
      return false
    } finally {
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

        <div className="console-new__modes" role="radiogroup" aria-label={t('console.new.title')}>
          {MODES.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="radio"
              aria-checked={mode === entry.id}
              className={`console-mode${mode === entry.id ? ' is-selected' : ''}`}
              onClick={() => setMode(entry.id)}
            >
              <span className="console-mode__top">
                <span className="console-mode__icon"><Icon name={entry.icon} size={14} /></span>
                {t(entry.labelKey)}
              </span>
              <p>{t(entry.hintKey)}</p>
            </button>
          ))}
        </div>

        {/*
          The session composer, unkeyed: the new-task page never remounts it on
          a thread switch (the page owns the draft under NEW_TASK_DRAFT_KEY and
          the staged overrides live in `staged`), so pills and attachments keep
          their state while the user types.
        */}
        <Composer
          threadId={null}
          onPendingStart={start}
          stagedRuntime={staged}
          onStagedChange={stage}
        />

        <div className="console-new__recent">
          <Icon name="clock" size={12} />
          {t('console.new.planHint')}
        </div>
      </div>
    </section>
  )
}
