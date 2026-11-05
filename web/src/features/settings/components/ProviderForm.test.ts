import { describe, expect, it } from 'vitest'
import type { ProviderModelSummary } from '../../../shared/contracts'
import { buildEntry, emptyDraft } from './ProviderDraft'
import { applyCatalogModel } from './ProviderCatalog'

describe('provider model discovery form helpers', () => {
  it('applies explicit catalog capabilities to the entry the user saves', () => {
    const draft = {
      ...emptyDraft(),
      id: 'gateway',
      base_url: 'https://gateway.example/v1',
      api_key: 'secret'
    }
    const model: ProviderModelSummary = {
      id: 'vision-model',
      context_window_tokens: 128_000,
      supports_vision: true,
      max_output_tokens: 16_384
    }

    const chosen = applyCatalogModel(draft, model)
    const built = buildEntry(chosen, false)

    expect(chosen.model).toBe('vision-model')
    expect(chosen.context_window_tokens).toBe('128000')
    expect(chosen.supports_vision).toBe(true)
    expect(built).toMatchObject({
      ok: true,
      entry: {
        model: 'vision-model',
        context_window_tokens: 128_000,
        supports_vision: true
      }
    })
  })

  it('keeps hand-typed capabilities when the catalog does not declare them', () => {
    const draft = {
      ...emptyDraft(),
      id: 'gateway',
      base_url: 'https://gateway.example/v1',
      api_key: 'secret',
      context_window_tokens: '200000',
      supports_vision: false
    }

    // A bare `{ id }` row is what most `/models` endpoints actually return.
    const chosen = applyCatalogModel(draft, { id: 'plain-model' })

    expect(chosen.model).toBe('plain-model')
    expect(chosen.context_window_tokens).toBe('200000')
    expect(chosen.supports_vision).toBe(false)
  })
})
