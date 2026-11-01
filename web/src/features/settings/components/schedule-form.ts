/**
 * Schedule form state: pure mapping between a draft and the wire requests.
 *
 * No React and no I/O here, so the payload a form submits can be asserted
 * directly in a unit test. Validation mirrors the runtime's *rejecting* rules
 * only (non-empty prompt, numeric interval, a real `HH:mm`, a chosen weekday);
 * normalization that the runtime performs silently (09:00 fallback, Monday
 * fallback, clamping) stays a server decision and is never duplicated.
 */
import { translateNow } from '../../../shared/i18n/current'
import type {
  CreateScheduleRequest,
  ScheduleKind,
  ScheduledTask,
  TaskSchedule,
  UpdateScheduleRequest
} from '../../../shared/contracts'

export const MIN_EVERY_MINUTES = 1

/**
 * Value shown when a task carries none: the runtime's own default for
 * unattended runs (`DEFAULT_SCHEDULE_AUTONOMY`), so the form never shows an
 * empty fourth state the contract does not have.
 */
export const DEFAULT_AUTONOMY = 'unattended'
export const MAX_EVERY_MINUTES = 525_600

/** Wire autonomy ids; empty string means "runtime default" (no override). */
export type ScheduleDraft = {
  title: string
  prompt: string
  kind: ScheduleKind
  everyMinutes: string
  timeOfDay: string
  /** `datetime-local` value for the one-shot cadence; converted on submit. */
  atTime: string
  dayOfWeek: number[]
  dayOfMonth: number[]
  autonomy: string
  model: string
  providerId: string
  workspace: string
}

export type BuiltRequest<T> = { ok: true; request: T } | { ok: false; error: string }

/** A blank draft: daily at 09:00, matching the runtime's own default. */
export function emptyScheduleDraft(): ScheduleDraft {
  return {
    title: '',
    prompt: '',
    kind: 'daily',
    everyMinutes: '30',
    timeOfDay: '09:00',
    atTime: '',
    dayOfWeek: [1],
    dayOfMonth: [1],
    autonomy: 'unattended',
    model: '',
    providerId: '',
    workspace: ''
  }
}

/** RFC3339 (UTC) → `datetime-local`, which `Date` reads as local time. */
export function toDateTimeLocal(iso: string): string {
  const date = new Date(iso.trim())
  if (Number.isNaN(date.getTime())) return ''
  const pad = (value: number): string => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/** `datetime-local` → RFC3339, so the runtime stores an unambiguous instant. */
export function fromDateTimeLocal(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return date.toISOString()
}

export function draftFromTask(task: ScheduledTask): ScheduleDraft {
  return {
    title: task.title,
    prompt: task.prompt,
    kind: task.schedule.kind,
    everyMinutes: String(task.schedule.every_minutes),
    timeOfDay: task.schedule.time_of_day || '09:00',
    atTime: toDateTimeLocal(task.schedule.at_time),
    dayOfWeek: task.schedule.day_of_week,
    dayOfMonth: task.schedule.day_of_month,
    autonomy: task.autonomy || DEFAULT_AUTONOMY,
    model: task.model ?? '',
    providerId: task.provider_id ?? '',
    workspace: task.workspace ?? ''
  }
}

/** Strict `HH:mm`: the runtime would silently replace anything else with 09:00. */
function isClock(value: string): boolean {
  return /^([01]\d|2[0-3]):[0-5]\d$/.test(value)
}

/** The cadence half of every request; fields for other kinds are preserved. */
export function buildSchedule(draft: ScheduleDraft): TaskSchedule {
  const minutes = Number(draft.everyMinutes.trim())
  return {
    kind: draft.kind,
    every_minutes: Number.isInteger(minutes) ? minutes : MIN_EVERY_MINUTES,
    time_of_day: draft.timeOfDay.trim(),
    at_time: draft.kind === 'at' ? fromDateTimeLocal(draft.atTime) : '',
    day_of_week: [...new Set(draft.dayOfWeek)].sort((left, right) => left - right),
    day_of_month: [...new Set(draft.dayOfMonth)].sort((left, right) => left - right)
  }
}

/** Client-side pre-check; the returned string is already translated copy. */
function validate(draft: ScheduleDraft): string | null {
  if (draft.prompt.trim() === '') return translateNow('settings.schedules.error.promptRequired')
  if (draft.kind === 'interval') {
    const minutes = Number(draft.everyMinutes.trim())
    if (!Number.isInteger(minutes) || minutes < MIN_EVERY_MINUTES || minutes > MAX_EVERY_MINUTES) {
      return translateNow('settings.schedules.error.intervalRange', {
        min: MIN_EVERY_MINUTES,
        max: MAX_EVERY_MINUTES
      })
    }
  }
  if (draft.kind === 'at' && fromDateTimeLocal(draft.atTime) === '') {
    return translateNow('settings.schedules.error.atRequired')
  }
  if ((draft.kind === 'daily' || draft.kind === 'weekly' || draft.kind === 'monthly') && !isClock(draft.timeOfDay.trim())) {
    return translateNow('settings.schedules.error.clockInvalid')
  }
  if (draft.kind === 'weekly' && draft.dayOfWeek.length === 0) {
    return translateNow('settings.schedules.error.weekdayRequired')
  }
  if (draft.kind === 'monthly' && draft.dayOfMonth.length === 0) {
    return translateNow('settings.schedules.error.monthDayRequired')
  }
  return null
}

/** Optional text fields are omitted when blank; empty means "runtime default". */
function optionalText(value: string): string | undefined {
  const trimmed = value.trim()
  return trimmed === '' ? undefined : trimmed
}

/** `POST /v1/schedules` body. */
export function buildCreateRequest(draft: ScheduleDraft): BuiltRequest<CreateScheduleRequest> {
  const error = validate(draft)
  if (error) return { ok: false, error }
  const request: CreateScheduleRequest = {
    prompt: draft.prompt.trim(),
    schedule: buildSchedule(draft)
  }
  const title = optionalText(draft.title)
  if (title) request.title = title
  const workspace = optionalText(draft.workspace)
  if (workspace) request.workspace = workspace
  // Autonomy is a three-way choice: the runtime default is an explicit value.
  request.autonomy = draft.autonomy || DEFAULT_AUTONOMY
  const model = optionalText(draft.model)
  if (model) request.model = model
  const providerId = optionalText(draft.providerId)
  if (providerId) request.provider_id = providerId
  return { ok: true, request }
}

/**
 * `PATCH /v1/schedules/{id}` body: only what the user actually changed.
 *
 * The runtime merges absent fields, so sending the whole task again would make
 * every edit touch every column (and rewrite `updated_at` bookkeeping for
 * fields nobody edited). An emptied optional field is sent as `""`, which is
 * the documented way to clear one.
 */
export function buildUpdateRequest(task: ScheduledTask, draft: ScheduleDraft): BuiltRequest<UpdateScheduleRequest> {
  const error = validate(draft)
  if (error) return { ok: false, error }
  const request: UpdateScheduleRequest = {}
  if (draft.title.trim() !== task.title) request.title = draft.title.trim()
  if (draft.prompt.trim() !== task.prompt) request.prompt = draft.prompt.trim()

  const workspace = draft.workspace.trim()
  if (workspace !== (task.workspace ?? '')) request.workspace = workspace
  // A stored task without an override runs as the default, so comparing the
  // preview value against that same default keeps the save a no-op.
  const autonomy = draft.autonomy || DEFAULT_AUTONOMY
  if (autonomy !== (task.autonomy || DEFAULT_AUTONOMY)) request.autonomy = autonomy
  const model = draft.model.trim()
  if (model !== (task.model ?? '')) request.model = model
  const providerId = draft.providerId.trim()
  if (providerId !== (task.provider_id ?? '')) request.provider_id = providerId

  const schedule = buildSchedule(draft)
  if (JSON.stringify(schedule) !== JSON.stringify(task.schedule)) request.schedule = schedule
  return { ok: true, request }
}

/** True when the PATCH would carry nothing, so the save button stays inert. */
export function isUpdateEmpty(request: UpdateScheduleRequest): boolean {
  return Object.keys(request).length === 0
}
