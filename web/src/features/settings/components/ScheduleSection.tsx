/**
 * Scheduled-task section: list, create/edit form, delete, run-now, master switch.
 *
 * Everything user-visible is catalog copy; the only identifiers are wire values
 * (`id`, `next_run_at`, autonomy ids). Cadence text and `next_run_at` formatting
 * come from `schedule-cadence` (display only — the runtime owns the arithmetic).
 *
 * A run opens a thread, so the row offers 「打开会话」: it selects the thread in
 * the existing code-session store and flips the app view back to the workbench,
 * which is the same selection path the thread list uses. A row whose run left no
 * thread shows the absence instead of a dead button.
 */
import { useEffect, useMemo, useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n, type MessageKey } from '../../../shared/i18n'
import type { ApplyResult, ScheduleKind, ScheduledTask } from '../../../shared/contracts'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'
import type { SettingsApi } from '../hooks/use-settings'
import { useSchedules } from '../hooks/use-schedules'
import { describeOutcome, type Outcome } from '../api/outcome'
import { CheckField } from './Field'
import { LoadError, Notice, SettingsSection, Skeleton } from './SectionShell'
import { ScheduleForm } from './ScheduleForm'
import {
  SCHEDULE_KIND_LABEL_KEYS,
  describeNextRun,
  describeSchedule,
  formatLocalStamp
} from './schedule-cadence'
import {
  buildCreateRequest,
  buildUpdateRequest,
  draftFromTask,
  emptyScheduleDraft,
  isUpdateEmpty,
  type ScheduleDraft
} from './schedule-form'

/** Mirrors `MAX_SCHEDULED_TASKS`; the runtime is still the authority that rejects. */
const SCHEDULE_LIMIT = 100

const STATUS_LABEL_KEYS: Record<string, MessageKey> = {
  running: 'settings.schedules.status.running',
  completed: 'settings.schedules.status.completed',
  failed: 'settings.schedules.status.failed',
  aborted: 'settings.schedules.status.aborted',
  skipped: 'settings.schedules.status.skipped'
}

function statusLabelKey(status: string): MessageKey {
  return STATUS_LABEL_KEYS[status] ?? 'settings.schedules.status.failed'
}

function TaskRow({
  task,
  busy,
  masterEnabled,
  onToggle,
  onEdit,
  onDelete,
  onRun,
  onOpenThread
}: {
  task: ScheduledTask
  busy: boolean
  masterEnabled: boolean
  onToggle: (next: boolean) => void
  onEdit: () => void
  onDelete: () => void
  onRun: () => void
  onOpenThread: (threadId: string) => void
}): JSX.Element {
  const { t, locale } = useI18n()
  const lastRun = task.last_run
  return (
    <div className="ark-card">
      <div className="ark-card__head">
        <span className="ark-card__name">{task.title || t('canvas.untitled')}</span>
        <span className="ark-chip">{t(SCHEDULE_KIND_LABEL_KEYS[task.schedule.kind as ScheduleKind] ?? 'settings.schedules.kind.manual')}</span>
        {task.enabled ? null : <span className="ark-chip ark-chip--muted">{t('settings.schedules.enabledOff')}</span>}
        <div className="ark-card__actions">
          <Button size="sm" disabled={busy || !masterEnabled} onClick={onRun}>
            {t('settings.schedules.run')}
          </Button>
          <Button size="sm" disabled={busy} onClick={onEdit}>
            {t('common.edit')}
          </Button>
          <Button variant="danger" size="sm" disabled={busy} onClick={onDelete}>
            {t('common.delete')}
          </Button>
        </div>
      </div>

      <div className="ark-card__meta">
        <span>{describeSchedule(task.schedule, t)}</span>
        <span>
          {t('settings.schedules.nextRun.label')}: {describeNextRun(task.next_run_at, t, locale)}
        </span>
      </div>

      <div className="ark-card__foot">
        <CheckField
          label={t('settings.schedules.enabledLabel')}
          checked={task.enabled}
          disabled={busy}
          onChange={onToggle}
        />
        <span className="ark-settings__hint">{t('settings.schedules.toggleHint')}</span>
      </div>

      <div className="ark-card__meta">
        <span>{t('settings.schedules.lastRun.label')}:</span>
        {lastRun ? (
          <>
            <span className={lastRun.status === 'failed' ? 'ark-settings__hint--error' : undefined}>
              {t('settings.schedules.lastRun.message', {
                status: t(statusLabelKey(lastRun.status)),
                time: formatLocalStamp(lastRun.finished_at ?? lastRun.started_at, locale)
              })}
            </span>
            {lastRun.message ? <span>{lastRun.message}</span> : null}
            {lastRun.thread_id ? (
              <button type="button" className="ark-linkbtn" onClick={() => onOpenThread(lastRun.thread_id as string)}>
                {t('settings.schedules.openThread')}
              </button>
            ) : (
              <span className="ark-settings__hint">{t('settings.schedules.threadMissing')}</span>
            )}
          </>
        ) : (
          <span className="ark-settings__hint">{t('settings.schedules.lastRun.none')}</span>
        )}
      </div>
    </div>
  )
}

export function ScheduleSection({ settings }: { settings: SettingsApi }): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const api = useSchedules()
  const { config, saving, save } = settings
  // Same selection the thread list uses; the app view switch then shows it.
  const selectThread = useCodeSessionStore((state) => state.setActiveThread)
  const setView = useViewStore((state) => state.setView)

  const [draft, setDraft] = useState<ScheduleDraft | null>(null)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const [rejection, setRejection] = useState<{ reason: string; restartRequired: string[] } | null>(null)
  const [masterEnabled, setMasterEnabled] = useState(false)

  // Adopt the runtime value on load and after each save, never fighting an edit.
  useEffect(() => {
    if (config) setMasterEnabled(config.schedules.enabled)
  }, [config])

  const editingTask = useMemo(
    () => (editingId ? api.tasks.find((task) => task.id === editingId) ?? null : null),
    [api.tasks, editingId]
  )
  const pendingUpdate = useMemo(
    () => (editingTask && draft ? buildUpdateRequest(editingTask, draft) : null),
    [draft, editingTask]
  )
  const unchanged = pendingUpdate?.ok === true && isUpdateEmpty(pendingUpdate.request)

  const persistMaster = async (next: boolean): Promise<void> => {
    setMasterEnabled(next)
    setRejection(null)
    const outcome: Outcome<ApplyResult> = await save({ schedules: { enabled: next } }, 'schedules')
    if (!outcome.ok) {
      setMasterEnabled(config?.schedules.enabled ?? false)
      toast.push('error', t('settings.schedules.master.saveFailed'), describeOutcome(outcome))
      return
    }
    if (outcome.value.result === 'rejected') {
      setMasterEnabled(config?.schedules.enabled ?? false)
      setRejection({ reason: outcome.value.reason, restartRequired: outcome.value.restart_required })
      toast.push('error', t('settings.schedules.master.rejected'), outcome.value.reason)
      return
    }
    toast.push('success', t('settings.schedules.master.succeeded'))
  }

  const toggleTask = async (task: ScheduledTask, next: boolean): Promise<void> => {
    const outcome = await api.update(task.id, { enabled: next })
    if (outcome.ok) {
      toast.push(
        'success',
        t('settings.schedules.toggled', {
          title: task.title,
          state: next ? t('settings.schedules.enabledOn') : t('settings.schedules.enabledOff')
        })
      )
      return
    }
    toast.push('error', t('settings.schedules.toggleFailed'), describeOutcome(outcome))
  }

  const removeTask = async (task: ScheduledTask): Promise<void> => {
    if (!window.confirm(t('settings.schedules.deleteConfirm', { title: task.title }))) return
    const outcome = await api.remove(task.id)
    if (outcome.ok) {
      if (editingId === task.id) {
        setDraft(null)
        setEditingId(null)
      }
      toast.push('success', t('settings.schedules.deleted', { title: task.title }))
      return
    }
    toast.push('error', t('settings.schedules.deleteFailed'), describeOutcome(outcome))
  }

  const runTask = async (task: ScheduledTask): Promise<void> => {
    const outcome = await api.runNow(task.id)
    if (outcome.ok) {
      toast.push('success', t('settings.schedules.runStarted', { title: task.title }))
      return
    }
    // 409 is the documented "still running / master switch off" refusal: the
    // client explains it in its own words and keeps the server sentence too.
    if (outcome.status === 409) {
      toast.push('info', t('settings.schedules.runConflict'), outcome.message)
      return
    }
    toast.push('error', t('settings.schedules.runFailed'), describeOutcome(outcome))
  }

  const openThread = (threadId: string): void => {
    selectThread(threadId)
    setView('code')
  }

  const submit = async (): Promise<void> => {
    if (!draft) return
    setFormError(null)
    if (editingTask) {
      const built = buildUpdateRequest(editingTask, draft)
      if (!built.ok) {
        setFormError(built.error)
        return
      }
      if (isUpdateEmpty(built.request)) {
        setFormError(t('settings.schedules.nothingChanged'))
        return
      }
      const outcome = await api.update(editingTask.id, built.request)
      if (!outcome.ok) {
        setFormError(describeOutcome(outcome))
        toast.push('error', t('settings.schedules.saveFailed'), describeOutcome(outcome))
        return
      }
      toast.push('success', t('settings.schedules.updated', { title: outcome.value.title }))
      setDraft(null)
      setEditingId(null)
      return
    }

    const built = buildCreateRequest(draft)
    if (!built.ok) {
      setFormError(built.error)
      return
    }
    const outcome = await api.create(built.request)
    if (!outcome.ok) {
      setFormError(describeOutcome(outcome))
      toast.push('error', t('settings.schedules.saveFailed'), describeOutcome(outcome))
      return
    }
    toast.push('success', t('settings.schedules.created', { title: outcome.value.title }))
    setDraft(null)
    setEditingId(null)
  }

  const body = ((): JSX.Element => {
    if ((api.status === 'loading' || api.status === 'idle') && api.tasks.length === 0) return <Skeleton rows={2} />
    if (api.status === 'error') {
      return <LoadError message={api.error ?? t('settings.schedules.loadFailed')} onRetry={() => void api.refresh()} />
    }
    if (api.tasks.length === 0) {
      return <EmptyState title={t('settings.schedules.emptyTitle')} hint={t('settings.schedules.emptyHint')} />
    }
    return (
      <div className="ark-settings__list">
        {api.tasks.map((task) => (
          <TaskRow
            key={task.id}
            task={task}
            busy={api.busy !== null}
            masterEnabled={masterEnabled}
            onToggle={(next) => void toggleTask(task, next)}
            onRun={() => void runTask(task)}
            onDelete={() => void removeTask(task)}
            onEdit={() => {
              setFormError(null)
              setEditingId(task.id)
              setDraft(draftFromTask(task))
            }}
            onOpenThread={openThread}
          />
        ))}
      </div>
    )
  })()

  const atLimit = api.tasks.length >= SCHEDULE_LIMIT

  return (
    <SettingsSection
      title={t('settings.schedules.title')}
      desc={config ? t('settings.schedules.desc') : t('settings.schedules.descLoading')}
      actions={
        <Button
          size="sm"
          disabled={api.busy !== null || atLimit}
          title={atLimit ? t('settings.schedules.capReached', { max: SCHEDULE_LIMIT }) : undefined}
          onClick={() => {
            setFormError(null)
            setEditingId(null)
            setDraft(emptyScheduleDraft())
          }}
        >
          {t('settings.schedules.add')}
        </Button>
      }
    >
      <div className="ark-card__foot">
        <CheckField
          label={t('settings.schedules.master.label')}
          checked={masterEnabled}
          disabled={saving === 'schedules' || !config}
          onChange={(next) => void persistMaster(next)}
        />
        <span className="ark-settings__hint">{t('settings.schedules.master.hint')}</span>
      </div>

      {rejection ? (
        <Notice tone="error" title={t('settings.schedules.master.rejectedTitle')}>
          <span>{t('settings.subagents.reason', { reason: rejection.reason })}</span>
          {rejection.restartRequired.length > 0 ? (
            <span>
              {t('settings.subagents.restartRequired', {
                keys: rejection.restartRequired.join(t('common.listSeparator'))
              })}
            </span>
          ) : (
            <span>{t('settings.subagents.restartNone')}</span>
          )}
        </Notice>
      ) : null}

      {body}

      {draft ? (
        <ScheduleForm
          draft={draft}
          editing={editingId !== null}
          busy={api.busy !== null}
          error={formError}
          unchanged={unchanged}
          onChange={setDraft}
          onSubmit={() => void submit()}
          onCancel={() => {
            setDraft(null)
            setEditingId(null)
            setFormError(null)
          }}
        />
      ) : null}
    </SettingsSection>
  )
}
