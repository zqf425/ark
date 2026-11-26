// @vitest-environment jsdom
/**
 * The provider section's save path over a mocked API surface.
 *
 * Pinned here: adding a provider never flips the global default
 * (`make_default` is always false), while the
 * row-level "Set as default" action still reaches `POST .../default`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import type { ProviderEntry, ProviderListResponse, ProviderView } from '../../../shared/contracts'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { deleteProvider, setDefaultProvider, upsertProvider } from '../api/providers-api'
import { ProvidersSection } from './ProvidersSection'
import type { ProvidersApi } from '../hooks/use-providers'

vi.mock('../api/providers-api', () => ({
  listProviders: vi.fn(),
  getProvider: vi.fn(),
  upsertProvider: vi.fn(),
  deleteProvider: vi.fn(),
  setDefaultProvider: vi.fn(),
  testProvider: vi.fn(),
  listProviderModels: vi.fn()
}))

const upsertMock = vi.mocked(upsertProvider)
const setDefaultMock = vi.mocked(setDefaultProvider)
const deleteMock = vi.mocked(deleteProvider)

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((ok) => { resolve = ok })
  return { promise, resolve }
}

function view(overrides: Partial<ProviderView> = {}): ProviderView {
  return {
    id: 'alpha',
    base_url: 'https://alpha.example',
    endpoint_format: 'openai_chat',
    model: 'alpha-model',
    enabled: true,
    is_default: true,
    has_api_key: true,
    api_key_hint: 'sk-1***ef',
    models: [],
    ...overrides
  }
}

function listResponse(providers: ProviderView[]): ProviderListResponse {
  const def = providers.find((row) => row.is_default)
  return {
    providers,
    default_provider_id: def?.id,
    active_provider_id: def?.id,
    compaction_threshold_tokens: 200_000
  }
}

/** The section receives the hook's state directly, so the mock is the hook. */
function providersApi(overrides: Partial<ProvidersApi> = {}): ProvidersApi {
  return {
    providers: [view()],
    defaultProviderId: 'alpha',
    defaultModel: null,
    activeProviderId: 'alpha',
    status: 'ready',
    error: null,
    busy: null,
    catalogBusy: false,
    refresh: vi.fn(async () => undefined),
    get: vi.fn(),
    /** Fold the raw response into the hook's `Outcome` shape. */
    upsert: async (request) => ({ ok: true, value: await upsertProvider(request) }),
    remove: async (providerId) => ({ ok: true, value: await deleteProvider(providerId) }),
    setDefault: async (providerId, model) => ({ ok: true, value: await setDefaultProvider(providerId, model) }),
    test: vi.fn(),
    models: vi.fn(),
    ...overrides
  }
}

describe('ProvidersSection default-provider semantics', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    upsertMock.mockReset()
    setDefaultMock.mockReset()
    deleteMock.mockReset()
  })

  afterEach(async () => {
    if (harness) await harness.unmount()
    harness = null
    vi.restoreAllMocks()
  })

  it('drops an edit read after the selected provider changes', async () => {
    const pending = deferred<{ ok: true; value: ProviderEntry }>()
    const api = providersApi({
      providers: [view(), view({ id: 'beta', is_default: false })],
      get: vi.fn(() => pending.promise)
    })
    harness = await mountWithProviders(<ProvidersSection providers={api} />)
    await act(async () => {
      [...harness!.host.querySelectorAll('button')].find((button) => button.textContent === '编辑')!.click()
    })
    await act(async () => {
      [...harness!.host.querySelectorAll<HTMLButtonElement>('.ark-provider-nav')].find((button) => button.textContent?.includes('beta'))!.click()
    })
    await act(async () => {
      pending.resolve({ ok: true, value: { ...view(), id: 'alpha', headers: {} } })
    })
    await settle()
    expect(harness.host.querySelector('#provider-id')).toBeNull()
    expect(harness.host.querySelector('.ark-provider-detail__title')?.textContent).toContain('beta')
  })

  it('seeds advanced edit fields from the full provider entry', async () => {
    harness = await mountWithProviders(<ProvidersSection providers={providersApi({
      get: vi.fn(async () => ({
        ok: true as const,
        value: {
          ...view(), headers: { 'X-Test': 'yes' }, max_tokens: 4096, temperature: 0.25
        }
      }))
    })} />)
    await act(async () => {
      [...harness!.host.querySelectorAll('button')].find((button) => button.textContent === '编辑')!.click()
    })
    await settle()
    await act(async () => {
      [...harness!.host.querySelectorAll('button')].find((button) => button.textContent?.includes('展开高级选项'))!.click()
    })
    expect(harness.host.querySelector<HTMLTextAreaElement>('#provider-headers')?.value).toContain('X-Test')
    expect(harness.host.querySelector<HTMLInputElement>('#provider-max-tokens')?.value).toBe('4096')
    expect(harness.host.querySelector<HTMLInputElement>('#provider-temperature')?.value).toBe('0.25')
  })

  it('adding a provider saves it with make_default=false, never switching the global default', async () => {
    upsertMock.mockResolvedValue(listResponse([view(), view({ id: 'beta', is_default: false })]))
    harness = await mountWithProviders(<ProvidersSection providers={providersApi()} />)

    // Open the create form and fill the required fields.
    const add = [...harness.host.querySelectorAll('button')].find((b) => b.textContent === '新增供应商')
    expect(add).toBeTruthy()
    await act(async () => {
      add!.click()
    })

    const set = (selector: string, value: string): void => {
      const input = harness!.host.querySelector<HTMLInputElement>(selector)
      expect(input, selector).toBeTruthy()
      const proto = input!.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
      Object.getOwnPropertyDescriptor(proto, 'value')?.set?.call(input!, value)
      input!.dispatchEvent(new Event('input', { bubbles: true }))
    }
    await act(async () => {
      set('#provider-id', 'beta')
      set('#provider-api-key', 'sk-beta-key')
      set('#provider-base-url', 'https://beta.example')
      set('#provider-model', 'beta-model')
    })
    set('#provider-id', 'beta')
    set('#provider-api-key', 'sk-beta-key')
    set('#provider-base-url', 'https://beta.example')
    set('#provider-model', 'beta-model')

    const save = [...harness.host.querySelectorAll('button')].find((b) => b.textContent === '保存')
    expect(save).toBeTruthy()
    await act(async () => {
      save!.click()
    })
    await settle()

    expect(upsertMock).toHaveBeenCalledTimes(1)
    const request = upsertMock.mock.calls[0]![0]
    expect(request.id).toBe('beta')
    // The regression: saving a provider is registration only. The default is a
    // deliberate switch and must never ride a create/edit submit.
    expect(request.make_default).toBe(false)
  })

  it('the create form offers no "set as default" checkbox at all', async () => {
    harness = await mountWithProviders(<ProvidersSection providers={providersApi()} />)
    const add = [...harness.host.querySelectorAll('button')].find((b) => b.textContent === '新增供应商')
    await act(async () => {
      add!.click()
    })
    const labels = [...harness.host.querySelectorAll('label.ark-check')].map((label) => label.textContent ?? '')
    expect(labels).not.toContain(expect.stringContaining('保存后设为默认'))
  })

  it('the row-level "Set as default" action still targets the default endpoint', async () => {
    setDefaultMock.mockResolvedValue(listResponse([view({ is_default: false }), view({ id: 'beta', is_default: true })]))
    harness = await mountWithProviders(
      <ProvidersSection
        providers={providersApi({
          providers: [view({ is_default: false }), view({ id: 'beta', is_default: true })],
          defaultProviderId: 'beta'
        })}
      />
    )

    // Set-as-default lives behind the detail header's "more actions" menu.
    const more = harness.host.querySelector<HTMLButtonElement>('[aria-label="更多操作"]')
    expect(more).toBeTruthy()
    await act(async () => {
      more!.click()
    })
    const makeDefault = [...harness.host.querySelectorAll('[role="menuitem"]')].find((b) => b.textContent === '设为默认') as HTMLButtonElement | undefined
    expect(makeDefault).toBeTruthy()
    await act(async () => {
      makeDefault!.click()
    })
    await settle()

    expect(setDefaultMock).toHaveBeenCalledWith('alpha', undefined)
  })

  it('skips disabled menu items, supports keyboard activation, and restores focus on Escape', async () => {
    setDefaultMock.mockResolvedValue(listResponse([view({ is_default: false }), view({ id: 'beta', is_default: true })]))
    harness = await mountWithProviders(<ProvidersSection providers={providersApi({
      providers: [view({ is_default: false }), view({ id: 'beta', is_default: true })], defaultProviderId: 'beta'
    })} />)
    const trigger = harness.host.querySelector<HTMLButtonElement>('[aria-label="更多操作"]')!
    await act(async () => { trigger.click() })
    const items = [...harness.host.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
    expect(document.activeElement).toBe(items[0])
    await act(async () => { items[0]!.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true })) })
    expect(setDefaultMock).toHaveBeenCalledWith('alpha', undefined)
  })

  it('cycles the menu with arrows, activates with Enter, and traps focus in the delete dialog', async () => {
    harness = await mountWithProviders(<ProvidersSection providers={providersApi()} />)
    const trigger = harness.host.querySelector<HTMLButtonElement>('[aria-label="更多操作"]')!
    await act(async () => { trigger.click() })
    const menuItems = [...harness.host.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
    expect(menuItems[0]?.disabled).toBe(true)
    expect(document.activeElement).toBe(menuItems[1])
    await act(async () => { menuItems[1]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true })) })
    expect(document.activeElement).toBe(menuItems[1])
    await act(async () => { menuItems[1]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', bubbles: true })) })
    expect(document.activeElement).toBe(menuItems[1])
    const documentEscape = vi.fn()
    document.addEventListener('keydown', documentEscape)
    try {
      await act(async () => { menuItems[1]!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })) })
    } finally {
      document.removeEventListener('keydown', documentEscape)
    }
    expect(documentEscape).not.toHaveBeenCalled()
    expect(harness.host.querySelector('[role="menu"]')).toBeNull()
    expect(document.activeElement).toBe(trigger)

    await act(async () => { trigger.click() })
    const deleteItem = harness.host.querySelector<HTMLButtonElement>('[role="menuitem"].is-danger')!
    await act(async () => { deleteItem.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })) })
    const dialog = harness.host.querySelector<HTMLDivElement>('[role="dialog"]')!
    const cancel = dialog.querySelector<HTMLButtonElement>('button')!
    const confirm = dialog.querySelectorAll<HTMLButtonElement>('button')[1]!
    expect(document.activeElement).toBe(cancel)
    await act(async () => { cancel.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })) })
    expect(document.activeElement).toBe(confirm)
    await act(async () => { confirm.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })) })
    expect(document.activeElement).toBe(cancel)
    await act(async () => { cancel.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true })) })
    expect(document.activeElement).toBe(confirm)
    await act(async () => { confirm.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })) })
    expect(harness.host.querySelector('[role="dialog"]')).toBeNull()
    expect(document.activeElement).toBe(trigger)

    await act(async () => { trigger.click() })
    expect(harness.host.querySelector('[role="menu"]')).not.toBeNull()
    await act(async () => { document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true })) })
    expect(harness.host.querySelector('[role="menu"]')).toBeNull()
  })

  it('deleting goes through a confirmation dialog before calling the api', async () => {
    deleteMock.mockResolvedValue(listResponse([]))
    harness = await mountWithProviders(<ProvidersSection providers={providersApi()} />)
    await act(async () => {
      harness!.host.querySelector<HTMLButtonElement>('[aria-label="更多操作"]')!.click()
    })
    const del = [...harness.host.querySelectorAll('[role="menuitem"]')].find((b) => b.textContent?.startsWith('删除')) as HTMLButtonElement
    await act(async () => {
      del.click()
    })
    expect(deleteMock).not.toHaveBeenCalled()
    const dialog = harness.host.querySelector('[role="dialog"]')
    expect(dialog?.textContent).toContain('默认供应商')
    const confirm = [...dialog!.querySelectorAll('button')].find((b) => b.textContent === '删除')!
    await act(async () => {
      confirm.click()
    })
    await settle()
    expect(deleteMock).toHaveBeenCalledWith('alpha')
  })
})
