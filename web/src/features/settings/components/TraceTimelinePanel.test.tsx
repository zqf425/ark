// @vitest-environment jsdom
/**
 * The single-turn timeline panel over its mocked API surface (§v2 验收).
 *
 * Pinned here: the panel is mounted only when its turn is expanded (so the
 * mount fetch *is* the click-time fetch — TraceSection's test pins that no
 * panel exists before the click), spans render top-down with kind labels,
 * a 404 renders the empty state instead of an error, failed spans carry the
 * failure mark, and a text span without `duration_ms` shows no duration.
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
        label: 'deepseek-v4.1'
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
    // Top-down in server order: kind labels come from the catalog, the label
    // field and snippets render next to them, real durations show.
    expect(harness.text()).toContain('用户消息')
    expect(harness.text()).toContain('模型调用')
    expect(harness.text()).toContain('deepseek-v4.1')
    expect(harness.text()).toContain('exec')
    expect(harness.text()).toContain('帮我修复登录')
    await harness.unmount()
  })

  it('shows durations for timed spans only', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const rows = [...harness.host.querySelectorAll('.ark-timeline__row')]
    expect(rows).toHaveLength(4)
    // The user_message span has no duration_ms: it renders as a tick, and its
    // timing readout carries no duration words.
    const userRow = rows.find((row) => row.getAttribute('data-span-kind') === 'user_message')
    expect(userRow?.querySelector('.ark-timeline__bar')?.className).toContain('is-tick')
    // The model_call span has one: a proportional bar plus duration words.
    const modelRow = rows.find((row) => row.getAttribute('data-span-kind') === 'model_call')
    expect(modelRow?.querySelector('.ark-timeline__bar')?.className).not.toContain('is-tick')
    expect(modelRow?.querySelector('.ark-timeline__when')?.textContent).toContain('3')
    await harness.unmount()
  })

  it('positions bars on the shared axis by start and duration share', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const bars = [...harness.host.querySelectorAll<HTMLElement>('.ark-timeline__bar')]
    expect(bars.length).toBe(4)
    for (const bar of bars) {
      const left = bar.style.getPropertyValue('--ark-span-left')
      expect(left).toMatch(/%$/)
      // Non-tick bars carry an explicit width; ticks rely on the fixed style.
      if (!bar.className.includes('is-tick')) {
        expect(bar.style.getPropertyValue('--ark-span-width')).toMatch(/%$/)
      }
    }
    // Ordering invariant: a later-starting span never sits left of an
    // earlier one on the shared axis.
    const lefts = bars.map((bar) => Number.parseFloat(bar.style.getPropertyValue('--ark-span-left')))
    // Fixture order is chronological, so each bar starts at or after the
    // previous one on the shared axis.
    for (let i = 1; i < lefts.length; i += 1) {
      const previous = lefts[i - 1] ?? 0
      expect(lefts[i] ?? 0).toBeGreaterThanOrEqual(previous)
    }
    await harness.unmount()
  })

  it('marks failed spans red', async () => {
    const harness = await mountWithProviders(<TraceTimelinePanel threadId={THREAD} turnId={TURN} />)
    await settle()

    const failedRow = harness.host.querySelector('.ark-timeline__row.is-failed')
    expect(failedRow).not.toBeNull()
    expect(failedRow?.getAttribute('data-span-kind')).toBe('tool_call')
    expect(failedRow?.textContent).toContain('失败')
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
