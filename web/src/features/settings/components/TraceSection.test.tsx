// @vitest-environment jsdom
/**
 * The trace section over its mocked API surface (requirement §验收).
 *
 * Pinned here: the default state never reaches a creation endpoint (the
 * button is the only trigger), the picker loads and enables the button once
 * stats arrive, a click runs create-then-turns in order, and empty stats
 * render the empty state instead of bare charts.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ThreadSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle } from '../../../shared/testing/react-harness'
import {
  createAnalysisThread,
  fetchTraceStats,
  listThreads,
  startAnalysisTurn,
  type TraceStatsResponse
} from '../api/trace-api'
import { TraceSection } from './TraceSection'

vi.mock('../api/trace-api', () => ({
  fetchTraceStats: vi.fn(),
  listThreads: vi.fn(),
  buildAnalysisPrompt: vi.fn(() => 'PROMPT'),
  analysisThreadTitle: vi.fn((title: string) => `AI分析-${title}`),
  createAnalysisThread: vi.fn(),
  startAnalysisTurn: vi.fn(),
  traceTokenTotals: vi.fn((usage: { prompt_tokens: number; completion_tokens: number; total_tokens: number } | undefined) => ({
    prompt_tokens: usage?.prompt_tokens ?? 0,
    completion_tokens: usage?.completion_tokens ?? 0,
    total_tokens: usage?.total_tokens ?? (usage?.prompt_tokens ?? 0) + (usage?.completion_tokens ?? 0)
  })),
  TRACE_PROMPT_JSON_LIMIT_BYTES: 8 * 1024
}))

const listMock = vi.mocked(listThreads)
const statsMock = vi.mocked(fetchTraceStats)
const createMock = vi.mocked(createAnalysisThread)
const turnMock = vi.mocked(startAnalysisTurn)

const THREAD_A = '00000000-0000-0000-0000-00000000000a'

function thread(): ThreadSummary {
  return {
    thread_id: THREAD_A,
    title: '修复登录',
    created_at: '2026-11-02T00:00:00Z',
    updated_at: '2026-11-02T01:00:00Z',
    archived: false,
    running: false
  }
}

function stats(overrides: Partial<TraceStatsResponse> = {}): TraceStatsResponse {
  return {
    thread_id: THREAD_A,
    turns: 2,
    active_ms: 65_000,
    model_calls: 5,
    usage: { prompt_tokens: 100, completion_tokens: 50, total_tokens: 150 },
    tool_calls: 4,
    failed_tool_calls: 1,
    active_days: 1,
    turn_stats: [
      { turn_id: '00000000-0000-0000-0000-000000000001', started_at: '2026-11-02T00:00:00Z', active_ms: 30_000, active: false, model_rounds: 2, tool_calls: 2, failed_tool_calls: 0 },
      { turn_id: '00000000-0000-0000-0000-000000000002', started_at: '2026-11-02T00:05:00Z', active_ms: 35_000, active: false, model_rounds: 3, tool_calls: 2, failed_tool_calls: 1 }
    ],
    tool_stats: [{ tool: 'read_file', calls: 3, failures: 0, avg_duration_ms: 120 }],
    slowest_calls: [{ seq: 9, turn_id: '00000000-0000-0000-0000-000000000002', call_id: 'c1', tool: 'exec', timestamp: '2026-11-02T00:06:00Z', duration_ms: 4_000, arguments_summary: 'ls -la' }],
    errors: [{ seq: 8, turn_id: '00000000-0000-0000-0000-000000000002', call_id: 'c0', tool: 'exec', timestamp: '2026-11-02T00:05:30Z', duration_ms: 200, detail: 'boom', errors: [] }],
    ...overrides
  }
}

async function pickFirstThread(host: HTMLElement): Promise<void> {
  const select = host.querySelector<HTMLSelectElement>('select')
  expect(select).toBeDefined()
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set
    setter?.call(select!, THREAD_A)
    select!.dispatchEvent(new Event('change', { bubbles: true }))
  })
  await settle()
}

describe('TraceSection', () => {
  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    statsMock.mockReset()
    createMock.mockReset()
    turnMock.mockReset()
    listMock.mockResolvedValue([thread()])
    statsMock.mockResolvedValue(stats())
    createMock.mockImplementation(async (title) => ({ ...thread(), thread_id: '00000000-0000-0000-0000-0000000000bb', title }))
    turnMock.mockResolvedValue({
      turn_id: '00000000-0000-0000-0000-0000000000cc',
      thread_id: '00000000-0000-0000-0000-0000000000bb',
      state: 'running',
      started_at: '2026-11-02T02:00:00Z'
    })
  })

  afterEach(async () => {
    document.body.innerHTML = ''
    vi.restoreAllMocks()
  })

  it('renders the picker and never calls a creation endpoint on mount', async () => {
    const harness = await mountWithProviders(<TraceSection />)
    await settle()

    expect(harness.text()).toContain('修复登录')
    // Default state is read-only: no POST path may fire before the user acts.
    expect(createMock).not.toHaveBeenCalled()
    expect(turnMock).not.toHaveBeenCalled()
    await harness.unmount()
  })

  it('enables the analyze button once the stats of the selected thread arrive', async () => {
    const harness = await mountWithProviders(<TraceSection />)
    await settle()

    const analyzeButton = () =>
      [...harness.host.querySelectorAll('button')].find((button) => button.textContent?.includes('启动 AI 分析'))
    expect(analyzeButton()?.disabled).toBe(true)

    await pickFirstThread(harness.host)

    expect(statsMock).toHaveBeenCalledWith(THREAD_A, expect.anything())
    expect(harness.text()).toContain('read_file')
    expect(analyzeButton()?.disabled).toBe(false)
    // Loading stats is still read-only.
    expect(createMock).not.toHaveBeenCalled()
    expect(turnMock).not.toHaveBeenCalled()
    await harness.unmount()
  })

  it('creates the analysis session then sends the prompt on click', async () => {
    const harness = await mountWithProviders(<TraceSection />)
    await settle()
    await pickFirstThread(harness.host)

    const analyzeButton = [...harness.host.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('启动 AI 分析')
    )
    await act(async () => {
      analyzeButton?.click()
    })
    await settle()

    expect(createMock).toHaveBeenCalledTimes(1)
    expect(createMock).toHaveBeenCalledWith('AI分析-修复登录')
    expect(turnMock).toHaveBeenCalledTimes(1)
    expect(turnMock).toHaveBeenCalledWith('00000000-0000-0000-0000-0000000000bb', 'PROMPT')
    expect(harness.text()).toContain('分析会话已创建')
    await harness.unmount()
  })

  it('renders the empty state for a thread without any events', async () => {
    statsMock.mockResolvedValue(
      stats({
        turns: 0,
        model_calls: 0,
        usage: undefined,
        tool_calls: 0,
        failed_tool_calls: 0,
        active_ms: undefined,
        turn_stats: [],
        tool_stats: [],
        slowest_calls: [],
        errors: []
      })
    )
    const harness = await mountWithProviders(<TraceSection />)
    await settle()
    await pickFirstThread(harness.host)

    expect(harness.text()).toContain('该会话没有工具调用记录')
    expect(harness.text()).toContain('该会话没有记录错误')
    // An empty projection still enables the (harmless) trigger, but nothing
    // was auto-created by rendering it.
    expect(createMock).not.toHaveBeenCalled()
    await harness.unmount()
  })

  it('shows the load error with a retry when stats fail', async () => {
    statsMock.mockRejectedValue(new Error('HTTP 500'))
    const harness = await mountWithProviders(<TraceSection />)
    await settle()
    await pickFirstThread(harness.host)

    expect(harness.text()).toContain('HTTP 500')
    expect(createMock).not.toHaveBeenCalled()
    await harness.unmount()
  })
})
