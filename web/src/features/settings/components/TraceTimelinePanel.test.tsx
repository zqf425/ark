// @vitest-environment jsdom
/**
 * The single-turn timeline panel over its mocked API surface (§v2 验收).
 *
 * Pinned here: the panel is mounted only when its turn is expanded (so the
 * mount fetch *is* the click-time fetch — TraceSection's test pins that no
 * panel exists before the click), spans render top-down with kind labels,
 * a 404 renders the empty state instead of an error, failed spans carry the
 * failure mark, reasoning/assistant text only appears inside an expanded
 * `model_call` row as ordered detail fragments (never as its own Gantt row),
 * and token/cache metrics only appear on expanded `model_call` rows with the
 * exact fields the server reported.
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle } from '../../../shared/testing/react-harness'
import { fetchTraceTimeline, type TraceTimeline } from '../api/trace-api'
import { TraceTimelinePanel } from './TraceTimelinePanel'

vi.mock('../api/trace-api', () => ({
  fetchTraceTimeline: vi.fn()
}))

const timelineMock = vi.mocked(fetchTraceTimeline)

const THREAD = '00000000-0000-0000-0000-00000000000a'
const TURN = '00000000-0000-0000-0000-000000000001'

function timeline(overrides: Partial<TraceTimeline> = {}): TraceTimeline {
  return {
    thread_id: THREAD,
    turn_id: TURN,
    spans: [
      {
        kind: 'user_message',
        started_at: '2026-11-02T00:00:00Z',
        text_snippet: '帮我修复登录'
      },
      {
        kind: 'model_call',
        started_at: '2026-11-02T00:00:05Z',
        duration_ms: 3_200,
        label: 'deepseek-v4.1',
        usage: {
          prompt_tokens: 1_234,
          completion_tokens: 567,
          total_tokens: 1_801,
          cache_hit_tokens: 900,
          cache_miss_tokens: 334,
          cache_hit_rate: 0.7294,
          cost_usd: 0.0123
        }
      },
      {
        kind: 'tool_call',
        started_at: '2026-11-02T00:00:10Z',
        duration_ms: 450,
        label: 'exec',
        text_snippet: 'ls -la'
      },
      {
        kind: 'tool_call',
        started_at: '2026-11-02T00:00:20Z',
        duration_ms: 1_500,
        label: 'read_file',
        is_error: true,
        text_snippet: 'boom'
      }
    ],
    ...overrides
  }
}

describe('TraceTimelinePanel', () => {
  beforeEach(() => {
    installBrowserStubs()
    timelineMock.mockReset()
    timelineMock.mockResolvedValue(timeline())
  })

  afterEach(async () => {
    document.body.innerHTML = ''
    vi.restoreAllMocks()
  })

  it('fetches once on mount (the mount is the click-time expansion) and renders the spans', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    expect(timelineMock).toHaveBeenCalledTimes(1)
    expect(timelineMock).toHaveBeenCalledWith(THREAD, TURN, expect.anything())
    // Top-down in server order: kind labels come from the catalog, the
    // label field renders in the row, real durations show in the duration
    // column. Snippets live in the expandable detail, not the collapsed row.
    expect(harness.text()).toContain('用户消息')
    expect(harness.text()).toContain('模型调用')
    expect(harness.text()).toContain('deepseek-v4.1')
    expect(harness.text()).toContain('exec')
    // The reference-style summary names the span count and total.
    expect(harness.text()).toContain('4 个 span')
    await harness.unmount()
  })

  it('renders model-call details and token/cache metrics in server order when expanded', async () => {
    const details = [
      { kind: 'reasoning' as const, started_at: '2026-11-02T00:00:06Z', text_snippet: '先检查登录链路' },
      { kind: 'assistant_text' as const, started_at: '2026-11-02T00:00:07Z', text_snippet: '我已经定位到问题' },
      { kind: 'reasoning' as const, started_at: '2026-11-02T00:00:08Z', text_snippet: '再确认修复方案' }
    ]
    timelineMock.mockResolvedValue(timeline({
      spans: [{
        kind: 'model_call',
        started_at: '2026-11-02T00:00:05Z',
        duration_ms: 3_200,
        details,
        usage: {
          prompt_tokens: 1_234,
          completion_tokens: 567,
          total_tokens: 1_801,
          cache_hit_tokens: 900,
          cache_miss_tokens: 334,
          cache_hit_rate: 0.7294,
          cost_usd: 0.0123
        }
      }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    // No independent reasoning/assistant_text Gantt rows exist.
    expect([...harness.host.querySelectorAll('.ark-gantt__row')].map((row) => row.getAttribute('data-span-kind'))).toEqual(['model_call'])
    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    expect(modelRow.parentElement?.querySelector('.ark-gantt__detail-section')).toBeNull()
    // Collapsed rows never show details or metrics.
    expect(harness.text()).not.toContain('输入 token')
    expect(harness.text()).not.toContain('缓存指标')

    await act(async () => {
      modelRow.click()
    })
    await settle()

    const detail = modelRow.parentElement!.querySelector('.ark-gantt__detail')!
    expect(detail.textContent).toContain('类型')
    expect(detail.textContent).toContain('开始')
    expect(detail.textContent).toContain('耗时')
    expect(detail.textContent).toContain('思考与回复')
    expect(detail.textContent).toContain('输入 token: 1,234')
    expect(detail.textContent).toContain('输出 token: 567')
    expect(detail.textContent).toContain('合计 token: 1,801')
    expect(detail.textContent).toContain('命中 token: 900')
    expect(detail.textContent).toContain('未命中 token: 334')
    expect(detail.textContent).toContain('命中率: 72.9%')
    expect(detail.textContent).not.toContain('供应商未上报缓存统计')
    const rows = [...detail.querySelectorAll('.ark-gantt__detail-row')]
    expect(rows).toHaveLength(3)
    expect(rows.map((row) => row.querySelector('.ark-gantt__detail-text')?.textContent)).toEqual(details.map((detailItem) => detailItem.text_snippet))
    expect(rows.map((row) => row.querySelector('.ark-gantt__detail-meta')?.textContent)).toEqual([
      expect.stringContaining('思考'),
      expect.stringContaining('助手回复'),
      expect.stringContaining('思考')
    ])
    await harness.unmount()
  })

  it('adds usage metrics to the fixture model call when its row is expanded', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    expect(harness.text()).not.toContain('输入 token')
    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => {
      modelRow.click()
    })
    await settle()
    const detail = modelRow.parentElement!.querySelector('.ark-gantt__detail')!
    expect(detail.textContent).toContain('输入 token: 1,234')
    expect(detail.textContent).toContain('命中率: 72.9%')
    await harness.unmount()
  })

  it('shows the localized unreported hint when the provider sent no cache fields', async () => {
    timelineMock.mockResolvedValue(timeline({
      spans: [{
        kind: 'model_call',
        started_at: '2026-11-02T00:00:05Z',
        duration_ms: 3_200,
        usage: { prompt_tokens: 12, completion_tokens: 34, total_tokens: 46 }
      }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => { modelRow.click() })
    await settle()
    const detail = modelRow.parentElement!.querySelector('.ark-gantt__detail')!
    expect(detail.textContent).toContain('输入 token: 12')
    expect(detail.textContent).toContain('供应商未上报缓存统计')
    expect(detail.textContent).not.toContain('命中 token')
    expect(detail.textContent).not.toContain('未命中 token')
    expect(detail.textContent).not.toContain('命中率')
    await harness.unmount()
  })

  it('renders only partial cache fields without computing a missing rate', async () => {
    timelineMock.mockResolvedValue(timeline({
      spans: [{
        kind: 'model_call',
        started_at: '2026-11-02T00:00:05Z',
        duration_ms: 3_200,
        usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120, cache_hit_tokens: 30 }
      }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => { modelRow.click() })
    await settle()
    const detail = modelRow.parentElement!.querySelector('.ark-gantt__detail')!
    expect(detail.textContent).toContain('命中 token: 30')
    expect(detail.textContent).not.toContain('未命中 token')
    expect(detail.textContent).not.toContain('命中率')
    expect(detail.textContent).not.toContain('供应商未上报缓存统计')
    await harness.unmount()
  })

  it('marks an approximate cache rate without recalculating it', async () => {
    timelineMock.mockResolvedValue(timeline({
      spans: [{
        kind: 'model_call',
        started_at: '2026-11-02T00:00:05Z',
        duration_ms: 3_200,
        usage: {
          prompt_tokens: 1_000,
          completion_tokens: 200,
          total_tokens: 1_200,
          cache_hit_tokens: 900,
          cache_hit_rate: 0.42,
          cache_hit_rate_approximate: true
        }
      }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => { modelRow.click() })
    await settle()
    const detail = modelRow.parentElement!.querySelector('.ark-gantt__detail')!
    expect(detail.textContent).toContain('命中率: 约 42%')
    expect(detail.textContent).toContain('命中 token: 900')
    expect(detail.textContent).not.toContain('未命中 token')
    await harness.unmount()
  })

  it('never renders token/cache sections on non-model rows', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    for (const kind of ['user_message', 'tool_call']) {
      const row = harness.host.querySelector<HTMLElement>(`.ark-gantt__row[data-span-kind="${kind}"]`)!
      await act(async () => { row.click() })
      await settle()
      expect(row.parentElement?.querySelector('.ark-gantt__detail')?.textContent).not.toContain('Token 用量')
      expect(row.parentElement?.querySelector('.ark-gantt__detail')?.textContent).not.toContain('缓存指标')
    }
    await harness.unmount()
  })

  it('hides the details section when a model call has no details', async () => {
    timelineMock.mockResolvedValue(timeline({
      spans: [{ kind: 'model_call', started_at: '2026-11-02T00:00:05Z', duration_ms: 3_200 }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => {
      modelRow.click()
    })
    await settle()

    expect(modelRow.parentElement?.querySelector('.ark-gantt__detail')).not.toBeNull()
    expect(modelRow.parentElement?.querySelector('.ark-gantt__detail-section')).toBeNull()
    expect(harness.text()).not.toContain('思考与回复')
    await harness.unmount()
  })

  it('shows the localized empty hint for an expanded model call with an empty details array', async () => {
    timelineMock.mockResolvedValue(timeline({
      spans: [{ kind: 'model_call', started_at: '2026-11-02T00:00:05Z', duration_ms: 3_200, details: [] }]
    }))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const modelRow = harness.host.querySelector<HTMLElement>('.ark-gantt__row[data-span-kind="model_call"]')!
    await act(async () => {
      modelRow.click()
    })
    await settle()

    const section = modelRow.parentElement?.querySelector('.ark-gantt__detail-section')
    expect(section?.textContent).toContain('本次模型调用没有留下思考或回复。')
    expect(section?.querySelectorAll('.ark-gantt__detail-row')).toHaveLength(0)
    await harness.unmount()
  })

  it('shows durations for timed spans only', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const rows = [...harness.host.querySelectorAll('.ark-gantt__row')]
    expect(rows).toHaveLength(4)
    // The user_message span has no duration_ms: the duration column reads —
    // and its bar carries no explicit width (CSS renders the 3px tick).
    const userRow = rows.find((row) => row.getAttribute('data-span-kind') === 'user_message')
    expect(userRow?.querySelector('.ark-gantt__duration')?.textContent).toContain('—')
    expect(userRow?.querySelector<HTMLElement>('.ark-gantt__bar')?.style.getPropertyValue('--ark-span-width')).toBe('')
    // The model_call span has one: the duration column carries the words and
    // the bar gets an explicit width share.
    const modelRow = rows.find((row) => row.getAttribute('data-span-kind') === 'model_call')
    expect(modelRow?.querySelector('.ark-gantt__duration')?.textContent).toContain('3')
    expect(modelRow?.querySelector<HTMLElement>('.ark-gantt__bar')?.style.getPropertyValue('--ark-span-width')).toMatch(/%$/)
    await harness.unmount()
  })

  it('positions bars on the shared axis by start and duration share', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const bars = [...harness.host.querySelectorAll<HTMLElement>('.ark-gantt__bar')]
    expect(bars.length).toBe(4)
    const lefts: number[] = []
    for (const bar of bars) {
      expect(bar.style.getPropertyValue('--ark-span-left')).toMatch(/%$/)
      const left = Number.parseFloat(bar.style.getPropertyValue('--ark-span-left'))
      lefts.push(Number.isNaN(left) ? 0 : left)
    }
    // Fixture order is chronological, so each bar starts at or after the
    // previous one on the shared axis.
    for (let i = 1; i < lefts.length; i += 1) {
      expect(lefts[i] ?? 0).toBeGreaterThanOrEqual(lefts[i - 1] ?? 0)
    }
    await harness.unmount()
  })

  it('marks failed spans red', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const failedRow = harness.host.querySelector('.ark-gantt__row.is-failed')
    expect(failedRow).not.toBeNull()
    expect(failedRow?.getAttribute('data-span-kind')).toBe('tool_call')
    // The failed mark lives in the expanded detail (reference layout); the
    // collapsed row's bar turns red via the is-failed CSS hook.
    expect(failedRow?.getAttribute('aria-expanded')).toBe('false')
    await harness.unmount()
  })

  it('renders the empty state on 404 (turn predates the timeline window)', async () => {
    timelineMock.mockResolvedValue(null)
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    expect(harness.text()).toContain('该回合没有可展示的时间线数据')
    // An empty turn is a value, not a failure: no alert surface.
    expect(harness.host.querySelector('[role="alert"]')).toBeNull()
    await harness.unmount()
  })

  it('renders the error state with the described failure for non-404 errors', async () => {
    timelineMock.mockRejectedValue(new Error('HTTP 500 boom'))
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    expect(harness.host.querySelector('[role="alert"]')).not.toBeNull()
    expect(harness.text()).toContain('HTTP 500 boom')
    await harness.unmount()
  })

  it('refetches when the expanded turn changes', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()
    await act(async () => {
      harness.render(<TraceTimelinePanel threadId={THREAD} turnId="00000000-0000-0000-0000-000000000002" />)
    })
    await settle()

    expect(timelineMock).toHaveBeenCalledTimes(2)
    expect(timelineMock).toHaveBeenLastCalledWith(THREAD, '00000000-0000-0000-0000-000000000002', expect.anything())
    await harness.unmount()
  })
})
