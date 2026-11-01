/**
 * The two request builders are the only place a form draft becomes wire JSON.
 *
 * `POST /v1/schedules` must carry a complete create body (prompt + cadence, and
 * only the optional fields the user actually filled in), while
 * `PATCH /v1/schedules/{id}` is a merge, so it must carry only the edited
 * fields and express "cleared" as an empty string. Both are asserted on the
 * literal payload, which is what the runtime receives.
 */
import { describe, expect, it } from 'vitest'
import type { ScheduledTask } from '../../../shared/contracts'
import {
  DEFAULT_AUTONOMY,
  buildCreateRequest,
  buildUpdateRequest,
  draftFromTask,
  emptyScheduleDraft,
  isUpdateEmpty
} from './schedule-form'

function task(patch: Partial<ScheduledTask> = {}): ScheduledTask {
  return {
    id: 'sch_1',
    title: 'morning report',
    enabled: true,
    prompt: 'summarize the inbox',
    schedule: {
      kind: 'daily',
      every_minutes: 0,
      time_of_day: '09:00',
      at_time: '',
      day_of_week: [],
      day_of_month: []
    },
    created_at: '2026-11-01T00:00:00Z',
    updated_at: '2026-11-01T00:00:00Z',
    next_run_at: '2026-11-02T01:00:00Z',
    history: [],
    ...patch
  }
}

describe('create request', () => {
  it('sends the prompt, the cadence, and the three-way autonomy choice', () => {
    const draft = { ...emptyScheduleDraft(), prompt: '  summarize the inbox  ', title: ' Inbox ' }
    const built = buildCreateRequest(draft)

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(built.request).toEqual({
      prompt: 'summarize the inbox',
      title: 'Inbox',
      autonomy: DEFAULT_AUTONOMY,
      schedule: {
        kind: 'daily',
        every_minutes: 30,
        time_of_day: '09:00',
        at_time: '',
        day_of_week: [1],
        day_of_month: [1]
      }
    })
  })

  it('omits blank optional fields instead of sending empty strings', () => {
    const draft = { ...emptyScheduleDraft(), prompt: 'nightly' }
    const built = buildCreateRequest(draft)

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(Object.keys(built.request).sort()).toEqual(['autonomy', 'prompt', 'schedule'])
  })

  it('keeps the cadence fields that belong to another kind', () => {
    const draft = {
      ...emptyScheduleDraft(),
      prompt: 'weekly review',
      kind: 'weekly' as const,
      timeOfDay: '20:00',
      dayOfWeek: [3, 1, 3],
      everyMinutes: '45'
    }
    const built = buildCreateRequest(draft)

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(built.request.schedule).toMatchObject({
      kind: 'weekly',
      time_of_day: '20:00',
      day_of_week: [1, 3],
      every_minutes: 45
    })
  })

  it('converts the one-shot local time into an RFC3339 instant', () => {
    const draft = { ...emptyScheduleDraft(), prompt: 'one shot', kind: 'at' as const, atTime: '2026-12-01T08:30' }
    const built = buildCreateRequest(draft)

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(new Date(built.request.schedule.at_time).toISOString()).toBe(built.request.schedule.at_time)
    expect(built.request.schedule.at_time).not.toBe('')
  })

  it('rejects the drafts the runtime would reject, before any request is sent', () => {
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: '   ' }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', kind: 'interval', everyMinutes: '0' }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', kind: 'interval', everyMinutes: 'abc' }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', kind: 'at' }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', timeOfDay: '9:00' }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', kind: 'weekly', dayOfWeek: [] }).ok).toBe(false)
    expect(buildCreateRequest({ ...emptyScheduleDraft(), prompt: 'x', kind: 'monthly', dayOfMonth: [] }).ok).toBe(false)
  })
})

describe('update request', () => {
  it('sends nothing when the user only opened and closed the form', () => {
    const stored = task()
    const built = buildUpdateRequest(stored, draftFromTask(stored))

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(built.request).toEqual({})
    expect(isUpdateEmpty(built.request)).toBe(true)
  })

  it('carries only the edited fields', () => {
    const stored = task()
    const built = buildUpdateRequest(stored, { ...draftFromTask(stored), timeOfDay: '10:00' })

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(Object.keys(built.request)).toEqual(['schedule'])
    expect(built.request.schedule).toMatchObject({ kind: 'daily', time_of_day: '10:00' })
  })

  it('expresses a cleared optional text field as an empty string', () => {
    const stored = task({ workspace: '/tmp/w', model: 'gpt', provider_id: 'openai' })
    const built = buildUpdateRequest(stored, { ...draftFromTask(stored), workspace: '', model: '', providerId: '' })

    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(built.request).toEqual({ workspace: '', model: '', provider_id: '' })
  })

  it('treats a missing stored autonomy as the default, so re-saving is a no-op', () => {
    const stored = task({ autonomy: undefined })
    expect(draftFromTask(stored).autonomy).toBe(DEFAULT_AUTONOMY)

    const built = buildUpdateRequest(stored, draftFromTask(stored))
    expect(built.ok).toBe(true)
    if (!built.ok) return
    expect(built.request).toEqual({})
  })
})
