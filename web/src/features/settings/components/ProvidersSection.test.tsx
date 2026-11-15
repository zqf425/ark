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
import type { ProviderListResponse, ProviderView } from '../../../shared/contracts'
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

    const row = harness.host.querySelectorAll('.ark-card')[0]!
    const makeDefault = [...row.querySelectorAll('button')].find((b) => b.textContent === '设为默认')
    expect(makeDefault).toBeTruthy()
    await act(async () => {
      makeDefault!.click()
    })
    await settle()

    expect(setDefaultMock).toHaveBeenCalledWith('alpha', undefined)
  })
})
