// @vitest-environment jsdom
/**
 * 端到端回归：会话详情播种 → 输入框 pill 标签。
 *
 * 用户报告的路径是"切换模型并发消息 → 切到别的会话 → 切回来，pill 又变回默认
 * 模型"。这条链路跨两个组件（`useThreadStream` 的 detail 播种 + `ModelPicker`
 * 读覆盖），单元测试各自为绿也无法发现。这里让真实的 stream hook 与真实的
 * pill 一起挂载，只替换 HTTP 边界。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ProviderView, ThreadDetail } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { getThread, getThreadUsage } from '../api/threads-api'
import { useCodeSessionStore } from '../state/store'
import { useThreadStream } from '../hooks/use-thread-stream'
import { ModelPicker } from '../components/ComposerRuntimeControls'

const THREAD_A = '00000000-0000-0000-0000-00000000000a'
const THREAD_B = '00000000-0000-0000-0000-00000000000b'

const DEFAULT_PROVIDER: ProviderView = {
  id: 'p1',
  label: 'Provider One',
  base_url: 'https://example.invalid/v1',
  endpoint_format: 'openai',
  model: 'default-model',
  enabled: true,
  is_default: true,
  has_api_key: true,
  models: [{ id: 'default-model' }]
}

const PINNED_PROVIDER: ProviderView = {
  id: 'p2',
  label: 'Provider Two',
  base_url: 'https://example.invalid/v1',
  endpoint_format: 'openai',
  model: 'model-b',
  enabled: true,
  is_default: false,
  has_api_key: true,
  models: [{ id: 'model-b' }]
}

/** The overrides each thread actually has stored server-side. */
const stored: Record<string, ThreadDetail['runtime']> = {
  [THREAD_A]: { provider_id: 'p2', model: 'model-b' },
  [THREAD_B]: {}
}

function detailOf(threadId: string): ThreadDetail {
  return {
    summary: {
      thread_id: threadId,
      title: threadId,
      created_at: '2026-11-01T00:00:00Z',
      updated_at: '2026-11-01T00:00:00Z',
      last_seq: 3,
      archived: false,
      running: false
    },
    items: [],
    child_ids: [],
    child_completions: [],
    attachment_ids: [],
    runtime: stored[threadId] ?? {},
    runtime_snapshot: { pending_approvals: [], pending_user_inputs: [] }
  }
}

vi.mock('../api/threads-api', () => ({
  getThread: vi.fn(),
  getThreadUsage: vi.fn(),
  updateThread: vi.fn(async () => ({})),
  compactThread: vi.fn()
}))
vi.mock('../api/events-api', () => ({ subscribeThreadEvents: vi.fn(() => new Promise<void>(() => undefined)) }))
vi.mock('../../settings/hooks/use-providers', () => ({
  useProviders: () => ({
    providers: [DEFAULT_PROVIDER, PINNED_PROVIDER],
    defaultProviderId: DEFAULT_PROVIDER.id,
    defaultModel: null,
    activeProviderId: DEFAULT_PROVIDER.id,
    status: 'ready',
    error: null,
    busy: null,
    catalogBusy: false,
    refresh: async () => undefined,
    get: async () => ({ ok: false as const }),
    upsert: async () => ({ ok: false as const }),
    remove: async () => ({ ok: false as const }),
    setDefault: async () => ({ ok: false as const }),
    test: async () => ({ ok: false as const }),
    models: async () => ({ ok: false as const })
  })
}))

const getThreadMock = vi.mocked(getThread)

/** Composer stand-in: the stream hook plus the real pill, keyed like Composer. */
function Canvas({ threadId }: { threadId: string }): JSX.Element {
  useThreadStream(threadId)
  return <ModelPicker key={threadId} threadId={threadId} />
}

function modelLabel(host: HTMLElement): string {
  return host.querySelector('.ark-composer__pill--model .ark-composer__pill-label')?.textContent ?? ''
}

describe('the composer model pill after a session switch', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    getThreadMock.mockReset()
    getThreadMock.mockImplementation(async (threadId: string) => detailOf(threadId))
    vi.mocked(getThreadUsage).mockReturnValue(new Promise<never>(() => undefined))
    useCodeSessionStore.setState({ threadRuntime: {}, threads: [], activeThreadId: null })
    useCodeSessionStore.getState().resetItems()
  })

  afterEach(async () => {
    await harness?.unmount()
    harness = null
  })

  it('shows A\'s model again after switching to B and back', async () => {
    harness = await mountWithProviders(<Canvas threadId={THREAD_A} />)
    await settle()
    expect(modelLabel(harness.host)).toContain('model-b')

    // Switching sessions remounts the composer; the pill loses its local state.
    await harness.render(<Canvas threadId={THREAD_B} />)
    await settle()
    expect(modelLabel(harness.host)).toContain('default-model')

    // The reported regression: coming back used to show the default model.
    await harness.render(<Canvas threadId={THREAD_A} />)
    await settle()
    expect(modelLabel(harness.host)).toContain('model-b')
  })

  it('reuses the detail load it already has instead of refetching per switch', async () => {
    harness = await mountWithProviders(<Canvas threadId={THREAD_A} />)
    await settle()
    await harness.render(<Canvas threadId={THREAD_B} />)
    await settle()
    await harness.render(<Canvas threadId={THREAD_A} />)
    await settle()

    // One detail read per selection: the pill never issues its own transcript
    // fetch on top of the stream's.
    expect(getThreadMock.mock.calls.map((call) => call[0])).toEqual([THREAD_A, THREAD_B, THREAD_A])
  })
})