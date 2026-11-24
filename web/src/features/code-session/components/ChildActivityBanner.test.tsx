// @vitest-environment jsdom
/**
 * Background-child surfaces (Qoder-referenced layout): a banner above the
 * canvas lists the thread's active subagent runs (pulse + 后台 tag + label +
 * live elapsed), and a hint above the composer says the user can keep
 * chatting. Both read the same per-thread child-runs list; clicking a banner
 * row opens the inspector's subagents panel.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ChildRunListResponse, ChildRunSummary } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle } from '../../../shared/testing/react-harness'
import { useCodeSessionStore } from '../state/store'
import { listChildRuns } from '../api/child-runs-api'
import { ChildActivityBanner, ChildActivityHint } from './ChildActivityBanner'

vi.mock('../api/child-runs-api', () => ({
  listChildRuns: vi.fn()
}))

const listMock = vi.mocked(listChildRuns)

let threadSeq = 0

/** A fresh thread id per test: the hook's module cache is keyed by thread. */
function freshThread(): string {
  threadSeq += 1
  return `00000000-0000-0000-0000-00000000b${threadSeq.toString().padStart(3, '0')}`
}

function child(id: string, label: string, status: ChildRunSummary['status'] = 'running'): ChildRunSummary {
  return {
    id: `00000000-0000-0000-0000-00000000c${id}`,
    parent_thread_id: '00000000-0000-0000-0000-00000000000a',
    parent_turn_id: '00000000-0000-0000-0000-0000000000t1',
    label,
    status,
    created_at: '2026-11-24T00:00:00Z'
  }
}

function response(runs: ChildRunSummary[]): ChildRunListResponse {
  return {
    enabled: true,
    active: runs.filter((run) => run.status === 'running' || run.status === 'queued').length,
    child_runs: runs,
    aggregates: {
      queued: 0,
      running: 0,
      completed: 0,
      failed: 0,
      aborted: 0,
      tool_invocations: 0,
      usage: { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }
    }
  }
}

describe('ChildActivityBanner', () => {
  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('renders nothing when the thread has no active child runs', async () => {
    const thread = freshThread()
    listMock.mockResolvedValue(response([child('01', 'done', 'completed')]))
    const h = await mountWithProviders(<ChildActivityBanner threadId={thread} />)
    await settle()
    expect(h.host.querySelector('.ark-childbanner')).toBeNull()
  })

  it('lists active runs with the 后台 tag, label and live elapsed', async () => {
    const thread = freshThread()
    listMock.mockResolvedValue(response([child('02', '调研竞品'), child('03', 'done', 'completed')]))
    const h = await mountWithProviders(<ChildActivityBanner threadId={thread} />)
    await settle()
    const rows = h.host.querySelectorAll('.ark-childbanner__row')
    expect(rows.length).toBe(1)
    expect(rows[0]!.textContent).toContain('后台')
    expect(rows[0]!.textContent).toContain('调研竞品')
    expect(rows[0]!.textContent).toContain('子 Agent 正在执行')
    expect(rows[0]!.querySelector('.ark-dot--live')).not.toBeNull()
  })

  it('opens the subagents panel on click', async () => {
    const thread = freshThread()
    useCodeSessionStore.setState({ activePanel: 'changes', rightCollapsed: true })
    listMock.mockResolvedValue(response([child('04', '调研竞品')]))
    const h = await mountWithProviders(<ChildActivityBanner threadId={thread} />)
    await settle()
    const row = h.host.querySelector<HTMLButtonElement>('.ark-childbanner__row')!
    row.click()
    await settle()
    const state = useCodeSessionStore.getState()
    expect(state.activePanel).toBe('subagents')
    expect(state.rightCollapsed).toBe(false)
  })

  it('caps the rows and folds the rest into one more-entry', async () => {
    const thread = freshThread()
    listMock.mockResolvedValue(
      response([child('05', 'a'), child('06', 'b'), child('07', 'c'), child('08', 'd'), child('09', 'e')])
    )
    const h = await mountWithProviders(<ChildActivityBanner threadId={thread} />)
    await settle()
    expect(h.host.querySelectorAll('.ark-childbanner__row').length).toBe(3)
    const more = h.host.querySelector('.ark-childbanner__more')
    expect(more).not.toBeNull()
    expect(more!.textContent).toContain('2')
  })
})

describe('ChildActivityHint', () => {
  beforeEach(() => {
    installBrowserStubs()
    listMock.mockReset()
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('tells the user they can keep sending messages while children run', async () => {
    const thread = freshThread()
    listMock.mockResolvedValue(response([child('10', 'a'), child('11', 'b')]))
    const h = await mountWithProviders(<ChildActivityHint threadId={thread} />)
    await settle()
    const hint = h.host.querySelector('.ark-childhint')
    expect(hint).not.toBeNull()
    expect(hint!.textContent).toContain('2 个任务在后台运行')
    expect(hint!.textContent).toContain('你可以继续发消息')
  })

  it('renders nothing without active children', async () => {
    const thread = freshThread()
    listMock.mockResolvedValue(response([]))
    const h = await mountWithProviders(<ChildActivityHint threadId={thread} />)
    await settle()
    expect(h.host.querySelector('.ark-childhint')).toBeNull()
  })
})