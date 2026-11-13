// @vitest-environment jsdom
/**
 * The two turn budgets are the only tuning fields whose lower bound is `0` and
 * whose `0` means "unlimited". Pin the rendered input bounds, the hint text,
 * and the behavioural contract: `0` is a valid, submittable value while the new
 * upper bounds reject anything above them.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { RuntimeConfigView } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { SettingsApi } from '../hooks/use-settings'
import { TuningSection } from './TuningSection'

function config(): RuntimeConfigView {
  return {
    version: 1,
    port: 18790,
    data_dir: '/tmp/ark',
    log_level: 'info',
    default_autonomy: 'workspace_write',
    subagents: { enabled: true, max_concurrency: 2, auto_continue: false },
    skills: { enabled: true, extra_dirs: [] },
    schedules: { enabled: true },
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

function setValue(element: Element, value: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
  setter?.call(element, value)
  element.dispatchEvent(new Event('input', { bubbles: true }))
}

function saveButton(harness: ReactHarness): HTMLButtonElement | undefined {
  return [...harness.host.querySelectorAll('button')].find(
    (button) => button.textContent === '保存调优'
  ) as HTMLButtonElement | undefined
}

describe('TuningSection turn budgets', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('renders 0-1000 and 0-100000000 bounds with unlimited hints', async () => {
    harness = await mountWithProviders(<TuningSection settings={settingsApi()} />)
    await settle()

    const rounds = harness.host.querySelector('#tuning-max_rounds') as HTMLInputElement
    const tokens = harness.host.querySelector('#tuning-max_tokens') as HTMLInputElement
    expect(rounds.min).toBe('0')
    expect(rounds.max).toBe('1000')
    expect(tokens.min).toBe('0')
    expect(tokens.max).toBe('100000000')
    expect(harness.text()).toContain('0 表示不限制')
  })

  it('accepts 0 as unlimited and submits it', async () => {
    const save = vi.fn(async () => ({ ok: true as const, value: { result: 'applied' as const, version: 2, changed: [] } }))
    harness = await mountWithProviders(<TuningSection settings={settingsApi({ save })} />)
    await settle()

    const rounds = harness.host.querySelector('#tuning-max_rounds') as HTMLInputElement
    await act(async () => {
      setValue(rounds, '0')
    })
    const button = saveButton(harness)!
    expect(button.disabled).toBe(false)
    await act(async () => {
      button.click()
    })
    await settle()

    expect(save).toHaveBeenCalledWith(
      { tuning: expect.objectContaining({ max_rounds: 0 }) },
      'tuning'
    )
  })

  it('rejects values above the new upper bounds', async () => {
    harness = await mountWithProviders(<TuningSection settings={settingsApi()} />)
    await settle()

    const tokens = harness.host.querySelector('#tuning-max_tokens') as HTMLInputElement
    await act(async () => {
      setValue(tokens, '100000001')
    })
    expect(saveButton(harness)?.disabled).toBe(true)
    expect(harness.text()).toContain('存在无效调优值')
  })
})
