// @vitest-environment jsdom
/**
 * The list's enable switch is the only place a row mutates without opening the
 * form, so the payload it sends is pinned here: `PATCH /v1/schedules/{id}` with
 * `{ enabled }` and nothing else. A merge endpoint treats an accidental extra
 * field as a real edit, so "only the flag" is the contract this test defends.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { RuntimeConfigView, ScheduledTask } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { SettingsApi } from '../hooks/use-settings'
import { listSchedules, updateSchedule } from '../api/schedules-api'
import { ScheduleSection } from './ScheduleSection'

vi.mock('../api/schedules-api', () => ({
  listSchedules: vi.fn(),
  createSchedule: vi.fn(),
  updateSchedule: vi.fn(),
  deleteSchedule: vi.fn(),
  runScheduleNow: vi.fn()
}))

const listMock = vi.mocked(listSchedules)
const updateMock = vi.mocked(updateSchedule)

const TASK_ID = 'sch_00000000-0000-0000-0000-000000000001'

function task(patch: Partial<ScheduledTask> = {}): ScheduledTask {
  return {
    id: TASK_ID,
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

function config(enabled = true): RuntimeConfigView {
  return {
    version: 1,
    port: 18790,
    data_dir: '/tmp/ark',
    log_level: 'info',
    default_autonomy: 'workspace_write',
    subagents: { enabled: true, max_concurrency: 2, auto_continue: false },
    skills: { enabled: true, extra_dirs: [] },
    schedules: { enabled },
    im: { enabled: false },
    tuning: {
      max_rounds: 40,
      max_tokens: 1_000_000,
      max_tool_calls: 200,
      max_identical_calls: 3,
      keep_recent_messages: 20,
      chunk_buffer: 64,
      sse_heartbeat_secs: 15,
      sse_idle_timeout_secs: 300,
      tool_storm_window_size: 0,
      max_parallel_tool_calls: 0
    }
  }
}

function settingsApi(overrides: Partial<SettingsApi> = {}): SettingsApi {
  return {
    config: config(),
    status: 'ready',
    error: null,
    saving: null,
    refresh: vi.fn(async () => undefined),
    save: vi.fn(async () => ({ ok: true as const, value: { result: 'applied' as const, version: 2, changed: [] } })),
    ...overrides
  }
}

describe('ScheduleSection enable switch', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    updateMock.mockReset()
    listMock.mockResolvedValue({ tasks: [task()] })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('patches only the enabled flag when a task is switched off', async () => {
    updateMock.mockResolvedValue(task({ enabled: false }))
    harness = await mountWithProviders(<ScheduleSection settings={settingsApi()} />)
    await settle()

    // Index 0 is the master switch; the row's switch follows it.
    const boxes = [...harness.host.querySelectorAll('input[type="checkbox"]')]
    expect(boxes).toHaveLength(2)
    const rowSwitch = boxes[1] as HTMLInputElement
    expect(rowSwitch.checked).toBe(true)

    await act(async () => {
      rowSwitch.click()
    })
    await settle()

    expect(updateMock).toHaveBeenCalledTimes(1)
    expect(updateMock).toHaveBeenCalledWith(TASK_ID, { enabled: false })
  })

  it('keeps the row unchanged when the runtime rejects the patch', async () => {
    updateMock.mockRejectedValue(new Error('runtime unavailable'))
    harness = await mountWithProviders(<ScheduleSection settings={settingsApi()} />)
    await settle()

    const rowSwitch = [...harness.host.querySelectorAll('input[type="checkbox"]')][1] as HTMLInputElement
    await act(async () => {
      rowSwitch.click()
    })
    await settle()

    expect(updateMock).toHaveBeenCalledWith(TASK_ID, { enabled: false })
    // No optimistic flip: the checkbox still reflects the stored task.
    expect(
      ([...harness.host.querySelectorAll('input[type="checkbox"]')][1] as HTMLInputElement).checked
    ).toBe(true)
  })
})
