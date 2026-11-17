// @vitest-environment jsdom
/**
 * 输入框 pill 只显示"本会话"的 runtime 覆盖。
 *
 * 覆盖存在会话 store 里（按 thread id 键），唯一的写入方是 `useThreadStream`
 * 的 detail 播种。这里直接播种 store，钉住 pill 的两条不变量：
 * 1) 会话有覆盖时，标签在"不打开菜单"的情况下就显示它（切回来同样如此）；
 * 2) 会话没有覆盖（或还没读到）时，标签显示运行时默认，绝不残留上一个会话的值。
 * 真实链路（detail 播种 → 标签）由 `hooks/thread-switch-model-pill.test.tsx` 覆盖。
 */
import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import type { ProviderView } from '../../../shared/contracts'
import { useCodeSessionStore } from '../state/store'
import { AutonomyPicker, ModelPicker } from './ComposerRuntimeControls'

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

vi.mock('../api/threads-api', () => ({
  getThread: vi.fn(async () => ({ runtime: {} })),
  updateThread: vi.fn(async () => ({})),
  compactThread: vi.fn()
}))

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

function modelPill(host: HTMLElement): string {
  return host.querySelector('.ark-composer__pill--model .ark-composer__pill-label')?.textContent ?? ''
}

function autonomyPill(host: HTMLElement): string {
  return host.querySelector('.ark-composer__pill--autonomy .ark-composer__pill-label')?.textContent ?? ''
}

describe('composer runtime pills read the selected session only', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    // A is pinned to another provider/model; B stores nothing.
    useCodeSessionStore.setState({
      threadRuntime: {
        [THREAD_A]: { provider_id: 'p2', model: 'model-b', autonomy: 'strict' },
        [THREAD_B]: {}
      }
    })
  })

  afterEach(async () => {
    useCodeSessionStore.setState({ threadRuntime: {} })
    await harness?.unmount()
    harness = null
  })

  it('shows the stored override without opening the menu, and keeps it after switching away and back', async () => {
    harness = await mountWithProviders(<ModelPicker key={THREAD_A} threadId={THREAD_A} />)
    await settle()
    expect(modelPill(harness.host)).toContain('model-b')

    // A real switch remounts the pill (keyed by thread), exactly like Composer.
    await harness.render(<ModelPicker key={THREAD_B} threadId={THREAD_B} />)
    await settle()
    expect(modelPill(harness.host)).toContain('default-model')

    // Back to A: the label must come from A's own stored overrides.
    await harness.render(<ModelPicker key={THREAD_A} threadId={THREAD_A} />)
    await settle()
    expect(modelPill(harness.host)).toContain('model-b')
  })

  it('follows threadId changes without a remount', async () => {
    harness = await mountWithProviders(<ModelPicker threadId={THREAD_A} />)
    await settle()
    expect(modelPill(harness.host)).toContain('model-b')

    await harness.render(<ModelPicker threadId={THREAD_B} />)
    await settle()
    expect(modelPill(harness.host)).toContain('default-model')

    await harness.render(<ModelPicker threadId={THREAD_A} />)
    await settle()
    expect(modelPill(harness.host)).toContain('model-b')
  })

  it('the autonomy pill follows the same rule', async () => {
    harness = await mountWithProviders(<AutonomyPicker key={THREAD_A} threadId={THREAD_A} />)
    await settle()
    expect(autonomyPill(harness.host)).toBe('需批准')

    await harness.render(<AutonomyPicker key={THREAD_B} threadId={THREAD_B} />)
    await settle()
    expect(autonomyPill(harness.host)).not.toBe('需批准')

    await harness.render(<AutonomyPicker key={THREAD_A} threadId={THREAD_A} />)
    await settle()
    expect(autonomyPill(harness.host)).toBe('需批准')
  })

  it('never paints the previous session override while the next one has none', async () => {
    harness = await mountWithProviders(<ModelPicker threadId={THREAD_A} />)
    await settle()
    expect(modelPill(harness.host)).toContain('model-b')

    // An unread thread (no store entry at all) must also fall back to the
    // runtime default instead of keeping A's value.
    await act(async () => {
      await harness!.render(<ModelPicker threadId={'00000000-0000-0000-0000-00000000000c'} />)
    })
    expect(modelPill(harness.host)).not.toContain('model-b')
  })
})