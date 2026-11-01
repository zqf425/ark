/**
 * Scheduled-task surface (`/v1/schedules`, requirements/scheduled-tasks.md).
 *
 * Only mapping: cadence normalization, `next_run_at` and the 100-task cap stay
 * in `ark-schedule`, so this module never re-derives a rule the runtime already
 * owns. Paths live here so no component carries a route of its own.
 */
import type {
  CreateScheduleRequest,
  ScheduledTask,
  ScheduledTaskListResponse,
  UpdateScheduleRequest
} from '../../../shared/contracts'
import { del, get, patch, post } from '../../../shared/api/client'

/** `GET /v1/schedules` — the whole task table, most recently updated first. */
export function listSchedules(signal?: AbortSignal): Promise<ScheduledTaskListResponse> {
  return get<ScheduledTaskListResponse>('/v1/schedules', signal ? { signal } : {})
}

/** `POST /v1/schedules` — create one task; the runtime assigns the id. */
export function createSchedule(request: CreateScheduleRequest): Promise<ScheduledTask> {
  return post<ScheduledTask>('/v1/schedules', request)
}

/**
 * `PATCH /v1/schedules/{id}` — merge an update.
 *
 * Every field is a merge, so an absent field keeps the stored value and an
 * empty string clears an optional text field.
 */
export function updateSchedule(taskId: string, request: UpdateScheduleRequest): Promise<ScheduledTask> {
  return patch<ScheduledTask>(`/v1/schedules/${encodeURIComponent(taskId)}`, request)
}

/** `DELETE /v1/schedules/{id}` — 204, or 404 when the task is already gone. */
export function deleteSchedule(taskId: string): Promise<void> {
  return del<void>(`/v1/schedules/${encodeURIComponent(taskId)}`)
}

/**
 * `POST /v1/schedules/{id}/run` — run once now, ignoring the cadence.
 *
 * 202 means the thread and its `running` marker already exist; 409 means the
 * previous run is still in flight (or the master switch is off).
 */
export function runScheduleNow(taskId: string): Promise<ScheduledTask> {
  return post<ScheduledTask>(`/v1/schedules/${encodeURIComponent(taskId)}/run`, {})
}
