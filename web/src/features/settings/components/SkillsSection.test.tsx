// @vitest-environment jsdom
/**
 * The skills config card over a stubbed SettingsApi.
 *
 * Pinned here: the card adopts the runtime values, the save button only fires
 * when the draft differs, the extra-dirs textarea maps to the `skills` patch
 * (one path per line), and a rejected save renders the runtime's reason.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import type { RuntimeConfigView } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { useSkills } from '../hooks/use-skills'
import type { SettingsApi } from '../hooks/use-settings'
import { SkillsSection } from './SkillsSection'

vi.mock('../api/skills-api', () => ({ listSkills: vi.fn() }))
vi.mock('../hooks/use-skills', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../hooks/use-skills')>()
  return { ...actual, useSkills: vi.fn() }
})

import { listSkills } from '../api/skills-api'

const listMock = vi.mocked(listSkills)
const useSkillsMock = vi.mocked(useSkills)

function config(): RuntimeConfigView {
  return {
    version: 1,
    port: 18790,
    data_dir: '/tmp/ark',
    log_level: 'info',
    default_autonomy: 'workspace_write',
    subagents: { enabled: true, max_concurrency: 2, auto_continue: false },
    skills: { enabled: true, extra_dirs: ['/tmp/skills-a'] },
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
  const proto = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  setter?.call(element, value)
  element.dispatchEvent(new Event('input', { bubbles: true }))
}

describe('SkillsSection config card', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    listMock.mockResolvedValue({
      enabled: true,
      roots: ['/root'],
      skills: [],
      validation_errors: []
    } as Awaited<ReturnType<typeof listSkills>>)
    useSkillsMock.mockReturnValue({
      enabled: true,
      roots: ['/root'],
      skills: [],
      validationErrors: [],
      status: 'ready',
      error: null,
      refresh: async () => undefined
    })
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('renders the runtime skills config as the draft', async () => {
    harness = await mountWithProviders(<SkillsSection settings={settingsApi()} />)
    await settle()

    const checkbox = harness.host.querySelector('input[type="checkbox"]') as HTMLInputElement
    expect(checkbox.checked).toBe(true)
    const textarea = harness.host.querySelector('textarea') as HTMLTextAreaElement
    expect(textarea.value).toBe('/tmp/skills-a')
  })

  it('saves only when dirty and submits the skills patch', async () => {
    const save = vi.fn(async () => ({ ok: true as const, value: { result: 'applied' as const, version: 2, changed: [] } }))
    harness = await mountWithProviders(<SkillsSection settings={settingsApi({ save })} />)
    await settle()

    const saveButton = [...harness.host.querySelectorAll('button')].find((button) => button.textContent === '保存')
    expect(saveButton?.disabled).toBe(true)

    const textarea = harness.host.querySelector('textarea')!
    await act(async () => {
      setValue(textarea, '/tmp/skills-a\n/tmp/skills-b')
    })
    expect(saveButton?.disabled).toBe(false)
    await act(async () => {
      saveButton?.click()
    })
    await settle()

    expect(save).toHaveBeenCalledWith(
      { skills: { enabled: true, extra_dirs: ['/tmp/skills-a', '/tmp/skills-b'] } },
      'skills'
    )
  })

  it('renders the runtime reason when the save is rejected', async () => {
    const save = vi.fn(async () => ({
      ok: true as const,
      value: { result: 'rejected' as const, reason: 'too many roots', restart_required: ['port'], version: 2, changed: [] }
    }))
    harness = await mountWithProviders(<SkillsSection settings={settingsApi({ save })} />)
    await settle()

    const checkbox = harness.host.querySelector('input[type="checkbox"]') as HTMLInputElement
    await act(async () => {
      checkbox.click()
    })
    await settle()

    expect(harness.text()).toContain('too many roots')
  })
})
